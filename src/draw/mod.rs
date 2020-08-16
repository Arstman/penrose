//! Utilities for rendering custom windows
pub mod bar;
pub mod text;

pub use inner::{Draw, DrawContext, WindowType, XCBDraw, XCBDrawContext};

mod inner {
    use std::collections::HashMap;

    use crate::core::data_types::WinId;

    use anyhow::anyhow;
    use cairo::{Context, XCBConnection, XCBDrawable, XCBSurface, XCBVisualType};
    use pango::{EllipsizeMode, FontDescription, Layout};
    use pangocairo::functions::{create_layout, show_layout};

    fn pango_layout(ctx: &Context) -> anyhow::Result<Layout> {
        create_layout(ctx).ok_or_else(|| anyhow!("unable to create pango layout"))
    }

    fn new_cairo_surface(
        conn: &xcb::Connection,
        screen: &xcb::Screen,
        window_type: &WindowType,
        width: i32,
        height: i32,
    ) -> anyhow::Result<(u32, XCBSurface)> {
        let id = create_window(conn, screen, window_type, width as u16, height as u16)?;
        let mut visualtype = get_visual_type(&conn, screen)?;

        let surface = unsafe {
            let conn_ptr = conn.get_raw_conn() as *mut cairo_sys::xcb_connection_t;

            XCBSurface::create(
                &XCBConnection::from_raw_none(conn_ptr),
                &XCBDrawable(id),
                &XCBVisualType::from_raw_none(
                    &mut visualtype.base as *mut xcb::ffi::xcb_visualtype_t
                        as *mut cairo_sys::xcb_visualtype_t,
                ),
                width,
                height,
            )
            .map_err(|err| anyhow!("Error creating surface: {}", err))?
        };

        surface.set_size(width, height).unwrap();
        Ok((id, surface))
    }

    fn get_visual_type(
        conn: &xcb::Connection,
        screen: &xcb::Screen,
    ) -> anyhow::Result<xcb::Visualtype> {
        conn.get_setup()
            .roots()
            .flat_map(|r| r.allowed_depths())
            .flat_map(|d| d.visuals())
            .find(|v| v.visual_id() == screen.root_visual())
            .ok_or_else(|| anyhow!("unable to get screen visual type"))
    }

    fn create_window(
        conn: &xcb::Connection,
        screen: &xcb::Screen,
        window_type: &WindowType,
        width: u16,
        height: u16,
    ) -> anyhow::Result<u32> {
        let id = conn.generate_id();

        xcb::create_window(
            &conn,
            xcb::COPY_FROM_PARENT as u8,
            id,
            screen.root(),
            0,
            0,
            width,
            height,
            0,
            xcb::WINDOW_CLASS_INPUT_OUTPUT as u16,
            0,
            &[
                (xcb::CW_BACK_PIXEL, screen.black_pixel()),
                (xcb::CW_EVENT_MASK, xcb::EVENT_MASK_EXPOSURE),
            ],
        );

        xcb::change_property(
            &conn,                                      // xcb connection to X11
            xcb::PROP_MODE_REPLACE as u8,               // discard current prop and replace
            id,                                         // window to change prop on
            intern_atom(&conn, "_NET_WM_WINDOW_TYPE")?, // prop to change
            intern_atom(&conn, "UTF8_STRING")?,         // type of prop
            8,                                          // data format (8/16/32-bit)
            window_type.as_ewmh_str().as_bytes(),       // data
        );

        xcb::map_window(&conn, id);
        conn.flush();

        Ok(id)
    }

    fn intern_atom(conn: &xcb::Connection, name: &str) -> anyhow::Result<u32> {
        xcb::intern_atom(conn, false, name)
            .get_reply()
            .map(|r| r.atom())
            .map_err(|err| anyhow!("unable to intern xcb atom '{}': {}", name, err))
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Color {
        r: f64,
        g: f64,
        b: f64,
    }
    impl Color {
        pub fn new_from_hex(hex: u32) -> Self {
            Self {
                r: ((hex & 0xFF0000) >> 16) as f64 / 255.0,
                g: ((hex & 0x00FF00) >> 8) as f64 / 255.0,
                b: (hex & 0x0000FF) as f64 / 255.0,
            }
        }

        pub fn rgb(&self) -> (f64, f64, f64) {
            (self.r, self.g, self.b)
        }
    }

    /// An EWMH Window type
    pub enum WindowType {
        /// A dock / status bar
        Dock,
        /// A menu
        Menu,
        /// A normal window
        Normal,
    }
    impl WindowType {
        pub(crate) fn as_ewmh_str(&self) -> &str {
            match self {
                WindowType::Dock => "_NET_WM_WINDOW_TYPE_DOCK",
                WindowType::Menu => "_NET_WM_WINDOW_TYPE_MENU",
                WindowType::Normal => "_NET_WM_WINDOW_TYPE_NORMAL",
            }
        }
    }

    /// A simple drawing abstraction
    pub trait Draw {
        /// The type of drawing context used for drawing
        type Ctx: DrawContext;

        /// Create a new client window with a canvas for drawing
        fn new_window(&mut self, t: &WindowType, w: usize, h: usize) -> anyhow::Result<WinId>;
        /// Get the size of the target screen in pixels
        fn screen_size(&self, ix: usize) -> anyhow::Result<(usize, usize)>;
        /// Register a font by name for later use
        fn register_font(&mut self, font_name: &str);
        /// Get a new DrawContext for the target window
        fn context_for(&self, id: WinId) -> anyhow::Result<Self::Ctx>;
        /// Flush pending actions
        fn flush(&self);
        /// Map the target window to the screen
        fn map_window(&self, id: WinId);
        /// Unmap the target window from the screen
        fn unmap_window(&self, id: WinId);
    }

    /// Used for simple drawing to the screen
    pub trait DrawContext {
        /// Set the active font, must have been registered on the partent Draw
        fn font(&mut self, font_name: &str, point_size: i32) -> anyhow::Result<&mut Self>;
        /// Set the color used for subsequent drawing operations
        fn color(&mut self, color: u32) -> &mut Self;
        /// Translate this context to (x, y) within the window
        fn translate(&self, x: f64, y: f64);
        /// Draw a filled rectangle using the current color
        fn rectangle(&self, x: f64, y: f64, w: f64, h: f64);
        /// Render 's' using the current font with the supplied padding. returns the extent taken
        /// up by the rendered text
        fn text(&self, s: &str, padding: (f64, f64, f64, f64)) -> anyhow::Result<(usize, usize)>;
    }

    /// An XCB based Draw
    pub struct XCBDraw {
        conn: xcb::Connection,
        fonts: HashMap<String, FontDescription>,
        surfaces: HashMap<WinId, cairo::XCBSurface>,
    }
    impl XCBDraw {
        /// Create a new empty XCBDraw. Fails if unable to connect to the X server
        pub fn new() -> anyhow::Result<Self> {
            let (conn, _) = xcb::Connection::connect(None)?;

            Ok(Self {
                conn,
                fonts: HashMap::new(),
                surfaces: HashMap::new(),
            })
        }

        fn screen(&self, ix: usize) -> anyhow::Result<xcb::Screen> {
            Ok(self
                .conn
                .get_setup()
                .roots()
                .nth(ix)
                .ok_or_else(|| anyhow!("Screen index out of bounds"))?)
        }
    }
    impl Draw for XCBDraw {
        type Ctx = XCBDrawContext;

        fn new_window(&mut self, t: &WindowType, w: usize, h: usize) -> anyhow::Result<WinId> {
            let screen = self.screen(0)?;
            let (id, surface) = new_cairo_surface(&self.conn, &screen, t, w as i32, h as i32)?;
            self.surfaces.insert(id, surface);

            Ok(id)
        }

        fn screen_size(&self, ix: usize) -> anyhow::Result<(usize, usize)> {
            let s = self.screen(ix)?;
            Ok((s.width_in_pixels() as usize, s.height_in_pixels() as usize))
        }

        fn register_font(&mut self, font_name: &str) {
            self.fonts
                .insert(font_name.into(), FontDescription::from_string(font_name));
        }

        fn context_for(&self, id: WinId) -> anyhow::Result<Self::Ctx> {
            let ctx = Context::new(
                self.surfaces
                    .get(&id)
                    .ok_or_else(|| anyhow!("uninitilaised window surface: {}", id))?,
            );

            Ok(XCBDrawContext {
                ctx,
                font: None,
                fonts: self.fonts.clone(),
            })
        }

        fn flush(&self) {
            self.conn.flush();
        }

        fn map_window(&self, id: WinId) {
            xcb::map_window(&self.conn, id);
        }

        fn unmap_window(&self, id: WinId) {
            xcb::unmap_window(&self.conn, id);
        }
    }

    /// An XCB based drawing context using pango and cairo
    pub struct XCBDrawContext {
        ctx: Context,
        font: Option<String>,
        fonts: HashMap<String, FontDescription>,
    }
    impl DrawContext for XCBDrawContext {
        fn font(&mut self, font_name: &str, point_size: i32) -> anyhow::Result<&mut Self> {
            let mut font = self
                .fonts
                .get_mut(font_name)
                .ok_or_else(|| anyhow!("unknown font: {}", font_name))?
                .clone();
            font.set_size(point_size * pango::SCALE);
            self.font = Some(font_name.to_string());

            Ok(self)
        }

        fn color(&mut self, color: u32) -> &mut Self {
            let (r, g, b) = Color::new_from_hex(color).rgb();
            self.ctx.set_source_rgb(r, g, b);

            self
        }

        fn translate(&self, x: f64, y: f64) {
            self.ctx.translate(x, y)
        }

        fn rectangle(&self, x: f64, y: f64, w: f64, h: f64) {
            self.ctx.rectangle(x, y, w, h);
            self.ctx.fill();
        }

        fn text(&self, s: &str, padding: (f64, f64, f64, f64)) -> anyhow::Result<(usize, usize)> {
            let layout = pango_layout(&self.ctx)?;
            if let Some(ref font) = self.font {
                layout.set_font_description(Some(self.fonts.get(font).unwrap()));
            }

            layout.set_text(s);
            layout.set_ellipsize(EllipsizeMode::End);

            let (w, h) = layout.get_pixel_size();
            layout.set_width(w as i32 * pango::SCALE);
            layout.set_height(h as i32 * pango::SCALE);

            let (l, r, t, b) = padding;
            self.ctx.translate(l, t);
            show_layout(&self.ctx, &layout);
            self.ctx.translate(-l, -t);

            let width = (w as f64 + l + r) as usize;
            let height = (h as f64 + t + b) as usize;
            Ok((width, height))
        }
    }
}