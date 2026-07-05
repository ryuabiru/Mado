use std::sync::Arc;

use anyhow::{Context, Result, bail};
use bytemuck::{Pod, Zeroable};
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, Style,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight, Wrap,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use wgpu::{
    BlendState, BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites,
    CommandEncoderDescriptor, CompositeAlphaMode, DeviceDescriptor, FragmentState, Instance,
    InstanceDescriptor, LoadOp, MultisampleState, Operations, PipelineCompilationOptions,
    PipelineLayoutDescriptor, PresentMode, PrimitiveState, RenderPassColorAttachment,
    RenderPassDescriptor, RenderPipeline, RenderPipelineDescriptor, RequestAdapterOptions,
    SurfaceConfiguration, TextureFormat, TextureUsages, TextureViewDescriptor, VertexState,
};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::config::FontConfig;
use crate::nvim::redraw::{CursorShape, CursorStyle};

use super::grid::{GridState, ResolvedHighlight};

const BASE_PADDING: f32 = 6.0;
const CELL_WIDTH_RATIO: f32 = 0.6;
const CELL_HEIGHT_RATIO: f32 = 22.0 / 15.0;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RectVertex {
    position: [f32; 2],
    color: [f32; 4],
}

struct PreparedGlyph {
    buffer: Buffer,
    left: f32,
    top: f32,
    color: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextFontRole {
    Primary,
    JapaneseFallback,
    IconFallback,
}

#[derive(Default)]
struct CachedGlyphRow {
    revision: Option<u64>,
    cursor_col: Option<usize>,
    glyphs: Vec<PreparedGlyph>,
}

pub struct Renderer {
    instance: Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: SurfaceConfiguration,
    rect_pipeline: RenderPipeline,
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    glyph_rows: Vec<CachedGlyphRow>,
    preedit_glyph: Option<PreparedGlyph>,
    rect_vertices: Vec<RectVertex>,
    rect_vertex_buffer: Option<wgpu::Buffer>,
    rect_vertex_capacity: u64,
    font_family: Option<String>,
    base_font_size: f32,
    japanese_font: Option<String>,
    icon_font: Option<String>,
    scale_factor: f64,
    background_alpha: f64,
    window: Arc<Window>,
}

impl Renderer {
    pub async fn new(
        window: Arc<Window>,
        event_loop: &ActiveEventLoop,
        font: &FontConfig,
        background_opacity: f32,
    ) -> Result<Self> {
        let size = nonzero_size(window.inner_size());
        let instance = Instance::new(InstanceDescriptor::new_with_display_handle(Box::new(
            event_loop.owned_display_handle(),
        )));
        let adapter = instance
            .request_adapter(&RequestAdapterOptions::default())
            .await
            .context("no compatible GPU adapter was found")?;
        let (device, queue) = adapter
            .request_device(&DeviceDescriptor::default())
            .await
            .context("failed to create GPU device")?;
        let surface = instance
            .create_surface(window.clone())
            .context("failed to create window surface")?;
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(TextureFormat::is_srgb)
            .or_else(|| capabilities.formats.first().copied())
            .context("window surface has no supported format")?;
        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode: PresentMode::Fifo,
            alpha_mode: composite_alpha_mode(&capabilities, background_opacity),
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let rect_pipeline = create_rect_pipeline(&device, format);
        let mut font_system = FontSystem::new();
        let font_family = find_font_family(&mut font_system, &font.family);
        if let Some(family) = &font_family {
            tracing::info!(family, size = font.size, "selected Mado font");
        } else {
            tracing::warn!(
                family = font.family,
                "configured font was not found; using platform monospace"
            );
        }
        let japanese_font = find_japanese_font(&mut font_system);
        if let Some(font) = &japanese_font {
            tracing::info!(font, "selected Japanese font");
        } else {
            tracing::warn!("no preferred Japanese font was found; using platform fallback");
        }
        let icon_font = find_icon_font(&mut font_system);
        if let Some(font) = &icon_font {
            tracing::info!(font, "selected icon font");
        } else {
            tracing::warn!("no Nerd Font was found; private-use icons may be unavailable");
        }
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, MultisampleState::default(), None);

        Ok(Self {
            instance,
            device,
            queue,
            surface,
            config,
            rect_pipeline,
            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            glyph_rows: Vec::new(),
            preedit_glyph: None,
            rect_vertices: Vec::new(),
            rect_vertex_buffer: None,
            rect_vertex_capacity: 0,
            font_family,
            base_font_size: font.size,
            japanese_font,
            icon_font,
            scale_factor: window.scale_factor(),
            background_alpha: f64::from(background_opacity),
            window,
        })
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>, scale_factor: f64) {
        if self.scale_factor != scale_factor {
            self.glyph_rows.clear();
        }
        self.scale_factor = scale_factor;
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn grid_dimensions(&self) -> (u64, u64) {
        let width = (self.config.width as f32 - self.padding() * 2.0).max(self.cell_width());
        let height = (self.config.height as f32 - self.padding() * 2.0).max(self.cell_height());
        (
            (width / self.cell_width()).floor().max(1.0) as u64,
            (height / self.cell_height()).floor().max(1.0) as u64,
        )
    }

    pub fn cell_at(&self, position: PhysicalPosition<f64>) -> (u64, u64) {
        let col = ((position.x as f32 - self.padding()).max(0.0) / self.cell_width()).floor();
        let row = ((position.y as f32 - self.padding()).max(0.0) / self.cell_height()).floor();
        (row as u64, col as u64)
    }

    pub fn pixel_scroll_lines(&self, horizontal: f64, vertical: f64) -> (f64, f64) {
        (
            horizontal / f64::from(self.cell_width()),
            vertical / f64::from(self.cell_height()),
        )
    }

    pub fn ime_cursor_area(
        &self,
        grid: &GridState,
        preedit: Option<(&str, Option<(usize, usize)>)>,
    ) -> (PhysicalPosition<f64>, PhysicalSize<u32>) {
        let cursor = grid.cursor();
        let available_columns = ime_available_columns(grid);
        let visible = preedit
            .filter(|(text, _)| !text.is_empty())
            .map(|(text, cursor)| visible_preedit(text, cursor, available_columns));
        let preedit_columns = visible
            .as_ref()
            .map(|visible| visible.selection_end.saturating_sub(visible.start_column))
            .unwrap_or(0);
        let x = self.padding() + (cursor.col + preedit_columns) as f32 * self.cell_width();
        let y = self.padding() + (cursor.row + 1) as f32 * self.cell_height();
        (
            PhysicalPosition::new(x as f64, y as f64),
            PhysicalSize::new(
                self.cell_width().ceil() as u32,
                self.cell_height().ceil() as u32,
            ),
        )
    }

    pub fn render(
        &mut self,
        grid: &GridState,
        preedit: Option<(&str, Option<(usize, usize)>)>,
        cursor_visible: bool,
    ) -> Result<()> {
        self.prepare_glyphs(grid, preedit, cursor_visible);
        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.config.width,
                height: self.config.height,
            },
        );

        let unsafe_glyphs = self
            .glyph_rows
            .iter()
            .flat_map(|row| &row.glyphs)
            .chain(self.preedit_glyph.iter())
            .filter(|glyph| !glyph_positions_are_safe(glyph))
            .count();
        if unsafe_glyphs > 0 {
            tracing::warn!(
                unsafe_glyphs,
                "skipping text with invalid glyph coordinates"
            );
        }
        let text_areas = self
            .glyph_rows
            .iter()
            .flat_map(|row| &row.glyphs)
            .chain(self.preedit_glyph.iter())
            .filter(|glyph| glyph_positions_are_safe(glyph))
            .map(|glyph| TextArea {
                buffer: &glyph.buffer,
                left: glyph.left,
                top: glyph.top,
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: 0,
                    right: self.config.width as i32,
                    bottom: self.config.height as i32,
                },
                default_color: glyph.color,
                custom_glyphs: &[],
            });
        self.text_renderer
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                text_areas,
                &mut self.swash_cache,
            )
            .context("failed to prepare glyphs")?;

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                self.window.request_redraw();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
                self.surface.configure(&self.device, &self.config);
                self.window.request_redraw();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self
                    .instance
                    .create_surface(self.window.clone())
                    .context("failed to recreate lost surface")?;
                self.surface.configure(&self.device, &self.config);
                self.window.request_redraw();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => bail!("wgpu surface validation error"),
        };

        let view = frame.texture.create_view(&TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("Mado frame encoder"),
            });
        let mut rect_vertices = std::mem::take(&mut self.rect_vertices);
        rect_vertices.clear();
        self.append_rectangle_vertices(grid, preedit, cursor_visible, &mut rect_vertices);
        self.rect_vertices = rect_vertices;
        self.ensure_rect_vertex_buffer();
        if let Some(buffer) = &self.rect_vertex_buffer
            && !self.rect_vertices.is_empty()
        {
            self.queue
                .write_buffer(buffer, 0, bytemuck::cast_slice(&self.rect_vertices));
        }

        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("Mado render pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(wgpu_color_with_alpha(
                            grid.background(),
                            self.background_alpha,
                        )),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some(buffer) = &self.rect_vertex_buffer
                && !self.rect_vertices.is_empty()
            {
                pass.set_pipeline(&self.rect_pipeline);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..self.rect_vertices.len() as u32, 0..1);
            }
            self.text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)
                .context("failed to render glyphs")?;
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();
        self.atlas.trim();
        Ok(())
    }

    fn prepare_glyphs(
        &mut self,
        grid: &GridState,
        preedit: Option<(&str, Option<(usize, usize)>)>,
        cursor_visible: bool,
    ) {
        let metrics = Metrics::new(self.font_size(), self.cell_height());
        let cell_width = self.cell_width();
        let cell_height = self.cell_height();
        let padding = self.padding();
        self.glyph_rows
            .resize_with(grid.height(), CachedGlyphRow::default);
        self.glyph_rows.truncate(grid.height());
        for row in 0..grid.height() {
            let cursor_col = (cursor_visible
                && grid.cursor_style().shape == CursorShape::Block
                && grid.cursor_is_on_main_grid()
                && grid.cursor().row == row)
                .then_some(grid.cursor().col);
            if self.glyph_rows[row].revision == Some(grid.row_revision(row))
                && self.glyph_rows[row].cursor_col == cursor_col
            {
                continue;
            }
            let mut glyphs = Vec::new();
            for col in 0..grid.width() {
                let Some(cell) = grid.cell(row, col) else {
                    continue;
                };
                if cell.text.is_empty() || cell.text == " " {
                    continue;
                }
                let highlight = grid.resolve_highlight(cell.highlight_id);
                let uses_unified_font = self
                    .font_family
                    .as_deref()
                    .is_some_and(is_unified_font_family);
                let role = font_role_for_text(
                    &cell.text,
                    uses_unified_font,
                    self.icon_font.is_some(),
                    self.japanese_font.is_some(),
                );
                let family = font_family_for_role(
                    role,
                    self.font_family.as_deref(),
                    self.japanese_font.as_deref(),
                    self.icon_font.as_deref(),
                );
                let attrs = text_attrs(highlight, family);
                let mut buffer = Buffer::new(&mut self.font_system, metrics);
                buffer.set_wrap(&mut self.font_system, Wrap::None);
                buffer.set_size(
                    &mut self.font_system,
                    Some(cell_width * 2.1),
                    Some(cell_height),
                );
                buffer.set_text(
                    &mut self.font_system,
                    &cell.text,
                    &attrs,
                    if role != TextFontRole::Primary || uses_unified_font {
                        Shaping::Basic
                    } else {
                        Shaping::Advanced
                    },
                    None,
                );
                buffer.shape_until_scroll(&mut self.font_system, false);
                glyphs.push(PreparedGlyph {
                    buffer,
                    left: padding + col as f32 * cell_width,
                    top: padding + row as f32 * cell_height,
                    color: glyphon_color(if cursor_col == Some(col) {
                        highlight.background
                    } else {
                        highlight.foreground
                    }),
                });
            }
            self.glyph_rows[row] = CachedGlyphRow {
                revision: Some(grid.row_revision(row)),
                cursor_col,
                glyphs,
            };
        }

        self.preedit_glyph = None;
        if let Some((text, cursor)) = preedit.filter(|(text, _)| !text.is_empty()) {
            let visible = visible_preedit(text, cursor, ime_available_columns(grid));
            if visible.text.is_empty() {
                return;
            }
            let mut buffer = Buffer::new(&mut self.font_system, metrics);
            buffer.set_wrap(&mut self.font_system, Wrap::None);
            buffer.set_size(
                &mut self.font_system,
                Some((visible.width.max(1) as f32 + 1.0) * cell_width),
                Some(cell_height),
            );
            let unified_font = self
                .font_family
                .as_deref()
                .filter(|family| is_unified_font_family(family));
            let family = unified_font
                .or(self.japanese_font.as_deref())
                .map(Family::Name)
                .unwrap_or(Family::Monospace);
            let attrs = Attrs::new().family(family).color(glyphon_color(0xffffff));
            let shaping = if unified_font.is_some() || self.japanese_font.is_some() {
                Shaping::Basic
            } else {
                Shaping::Advanced
            };
            buffer.set_text(&mut self.font_system, visible.text, &attrs, shaping, None);
            buffer.shape_until_scroll(&mut self.font_system, false);
            let cursor = grid.cursor();
            self.preedit_glyph = Some(PreparedGlyph {
                buffer,
                left: padding + cursor.col as f32 * cell_width,
                top: padding + cursor.row as f32 * cell_height,
                color: glyphon_color(0xffffff),
            });
        }
    }

    fn append_rectangle_vertices(
        &self,
        grid: &GridState,
        preedit: Option<(&str, Option<(usize, usize)>)>,
        cursor_visible: bool,
        vertices: &mut Vec<RectVertex>,
    ) {
        for (index, cell) in grid.cells().iter().enumerate() {
            let highlight = grid.resolve_highlight(cell.highlight_id);
            let row = index / grid.width().max(1);
            let col = index % grid.width().max(1);
            let x = self.padding() + col as f32 * self.cell_width();
            let y = self.padding() + row as f32 * self.cell_height();
            if highlight.background != grid.background() {
                self.push_background_rect(
                    vertices,
                    x,
                    y,
                    self.cell_width(),
                    self.cell_height(),
                    highlight.background,
                );
            }
            let line_thickness = snap_line_thickness(self.scale());
            if highlight.undercurl {
                for (segment_x, segment_y, segment_width, segment_height) in
                    undercurl_segments(x, y, self.cell_width(), self.cell_height(), self.scale())
                {
                    self.push_rect(
                        vertices,
                        segment_x,
                        segment_y,
                        segment_width,
                        segment_height,
                        highlight.special,
                    );
                }
            } else if highlight.underline {
                self.push_rect(
                    vertices,
                    x,
                    snap_to_device_pixel(
                        y + self.cell_height() - line_thickness * 2.0,
                        self.scale(),
                    ),
                    self.cell_width(),
                    line_thickness,
                    highlight.special,
                );
            }
            if highlight.strikethrough {
                self.push_rect(
                    vertices,
                    x,
                    snap_to_device_pixel(y + self.cell_height() * 0.55, self.scale()),
                    self.cell_width(),
                    line_thickness,
                    highlight.special,
                );
            }
            if highlight.overline {
                self.push_rect(
                    vertices,
                    x,
                    snap_to_device_pixel(y, self.scale()),
                    self.cell_width(),
                    line_thickness,
                    highlight.special,
                );
            }
        }

        if cursor_visible && grid.cursor_is_on_main_grid() && grid.cursor().row < grid.height() {
            let cursor = grid.cursor();
            let style = grid.cursor_style();
            let highlight = grid
                .cell(cursor.row, cursor.col)
                .map(|cell| grid.resolve_highlight(cell.highlight_id))
                .unwrap_or_else(|| grid.resolve_highlight(0));
            let color = cursor_color(grid, &style, highlight);
            let percentage = style.cell_percentage.clamp(1, 100) as f32 / 100.0;
            let cursor_width = self.cell_width() * cursor_cell_columns(grid) as f32;
            let (x_offset, y_offset, width, height) = match style.shape {
                CursorShape::Block => (0.0, 0.0, cursor_width, self.cell_height()),
                CursorShape::Vertical => {
                    (0.0, 0.0, self.cell_width() * percentage, self.cell_height())
                }
                CursorShape::Horizontal => (
                    0.0,
                    self.cell_height() * (1.0 - percentage),
                    cursor_width,
                    self.cell_height() * percentage,
                ),
            };
            self.push_rect(
                vertices,
                self.padding() + cursor.col as f32 * self.cell_width() + x_offset,
                self.padding() + cursor.row as f32 * self.cell_height() + y_offset,
                width,
                height,
                color,
            );
        }

        if let Some((text, preedit_cursor)) = preedit.filter(|(text, _)| !text.is_empty()) {
            let visible = visible_preedit(text, preedit_cursor, ime_available_columns(grid));
            if visible.text.is_empty() {
                return;
            }
            let grid_cursor = grid.cursor();
            let width = visible.width.max(1) as f32 * self.cell_width();
            let x = self.padding() + grid_cursor.col as f32 * self.cell_width();
            let y = self.padding() + grid_cursor.row as f32 * self.cell_height();
            self.push_background_rect(vertices, x, y, width, self.cell_height(), 0x3b4252);
            if visible.clipped_start {
                self.push_rect(
                    vertices,
                    x,
                    y + self.scale_factor as f32 * 2.0,
                    self.scale_factor as f32 * 2.0,
                    self.cell_height() - self.scale_factor as f32 * 4.0,
                    0x88c0d0,
                );
            }
            if visible.clipped_end {
                self.push_rect(
                    vertices,
                    x + width - self.scale_factor as f32 * 2.0,
                    y + self.scale_factor as f32 * 2.0,
                    self.scale_factor as f32 * 2.0,
                    self.cell_height() - self.scale_factor as f32 * 4.0,
                    0x88c0d0,
                );
            }
            if visible.selection_end >= visible.start_column {
                let start_col = visible.selection_start.saturating_sub(visible.start_column);
                let end_col = visible.selection_end.saturating_sub(visible.start_column);
                let selection_start = start_col.min(end_col) as f32 * self.cell_width();
                let selection_width = start_col.abs_diff(end_col) as f32 * self.cell_width();
                if selection_width > 0.0 {
                    self.push_background_rect(
                        vertices,
                        x + selection_start,
                        y,
                        selection_width,
                        self.cell_height(),
                        0x5e81ac,
                    );
                } else {
                    self.push_rect(
                        vertices,
                        x + selection_start,
                        y + self.scale_factor as f32 * 2.0,
                        self.scale_factor as f32,
                        self.cell_height() - self.scale_factor as f32 * 4.0,
                        0xeceff4,
                    );
                }
            }
            self.push_rect(
                vertices,
                x,
                y + self.cell_height() - self.scale_factor as f32 * 2.0,
                width,
                self.scale_factor as f32 * 2.0,
                0x88c0d0,
            );
        }
    }

    fn ensure_rect_vertex_buffer(&mut self) {
        let required = (self.rect_vertices.len() * std::mem::size_of::<RectVertex>()) as u64;
        if required == 0 || required <= self.rect_vertex_capacity {
            return;
        }
        let capacity = required.next_power_of_two();
        self.rect_vertex_buffer = Some(self.device.create_buffer(&BufferDescriptor {
            label: Some("Mado rectangle vertices"),
            size: capacity,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        self.rect_vertex_capacity = capacity;
    }

    fn push_rect(
        &self,
        vertices: &mut Vec<RectVertex>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: u32,
    ) {
        self.push_rect_vertices(vertices, x, y, width, height, linear_color(color, 1.0));
    }

    fn push_background_rect(
        &self,
        vertices: &mut Vec<RectVertex>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: u32,
    ) {
        self.push_rect_vertices(
            vertices,
            x,
            y,
            width,
            height,
            linear_color(color, self.background_alpha as f32),
        );
    }

    fn push_rect_vertices(
        &self,
        vertices: &mut Vec<RectVertex>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: [f32; 4],
    ) {
        let left = x / self.config.width as f32 * 2.0 - 1.0;
        let right = (x + width) / self.config.width as f32 * 2.0 - 1.0;
        let top = 1.0 - y / self.config.height as f32 * 2.0;
        let bottom = 1.0 - (y + height) / self.config.height as f32 * 2.0;
        vertices.extend([
            RectVertex {
                position: [left, top],
                color,
            },
            RectVertex {
                position: [left, bottom],
                color,
            },
            RectVertex {
                position: [right, bottom],
                color,
            },
            RectVertex {
                position: [left, top],
                color,
            },
            RectVertex {
                position: [right, bottom],
                color,
            },
            RectVertex {
                position: [right, top],
                color,
            },
        ]);
    }

    fn scale(&self) -> f32 {
        self.scale_factor as f32
    }

    fn font_size(&self) -> f32 {
        self.base_font_size * self.scale()
    }

    fn cell_width(&self) -> f32 {
        self.base_font_size * CELL_WIDTH_RATIO * self.scale()
    }

    fn cell_height(&self) -> f32 {
        (self.base_font_size * CELL_HEIGHT_RATIO).max(self.base_font_size) * self.scale()
    }

    fn padding(&self) -> f32 {
        BASE_PADDING * self.scale()
    }
}

fn create_rect_pipeline(device: &wgpu::Device, format: TextureFormat) -> RenderPipeline {
    let shader = device.create_shader_module(wgpu::include_wgsl!("rect.wgsl"));
    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("Mado rectangle pipeline layout"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4];
    device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("Mado rectangle pipeline"),
        layout: Some(&layout),
        vertex: VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: PipelineCompilationOptions::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<RectVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attributes,
            }],
        },
        primitive: PrimitiveState::default(),
        depth_stencil: None,
        multisample: MultisampleState::default(),
        fragment: Some(FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: PipelineCompilationOptions::default(),
            targets: &[Some(ColorTargetState {
                format,
                blend: Some(BlendState::ALPHA_BLENDING),
                write_mask: ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn text_attrs(highlight: ResolvedHighlight, family: Family<'_>) -> Attrs<'_> {
    let mut attrs = Attrs::new().family(family);
    if highlight.bold {
        attrs = attrs.weight(Weight::BOLD);
    }
    if highlight.italic {
        attrs = attrs.style(Style::Italic);
    }
    attrs
}

fn glyphon_color(rgb: u32) -> Color {
    Color::rgb(
        ((rgb >> 16) & 0xff) as u8,
        ((rgb >> 8) & 0xff) as u8,
        (rgb & 0xff) as u8,
    )
}

fn wgpu_color_with_alpha(rgb: u32, alpha: f64) -> wgpu::Color {
    wgpu::Color {
        r: srgb_to_linear(((rgb >> 16) & 0xff) as f32 / 255.0) as f64,
        g: srgb_to_linear(((rgb >> 8) & 0xff) as f32 / 255.0) as f64,
        b: srgb_to_linear((rgb & 0xff) as f32 / 255.0) as f64,
        a: alpha,
    }
}

fn linear_color(rgb: u32, alpha: f32) -> [f32; 4] {
    [
        srgb_to_linear(((rgb >> 16) & 0xff) as f32 / 255.0),
        srgb_to_linear(((rgb >> 8) & 0xff) as f32 / 255.0),
        srgb_to_linear((rgb & 0xff) as f32 / 255.0),
        alpha,
    ]
}

fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn cursor_color(grid: &GridState, style: &CursorStyle, cell: ResolvedHighlight) -> u32 {
    style
        .attr_id
        .filter(|id| *id != 0)
        .map(|id| grid.resolve_highlight(id).background)
        .unwrap_or(cell.foreground)
}

fn cursor_cell_columns(grid: &GridState) -> usize {
    let cursor = grid.cursor();
    grid.cell(cursor.row, cursor.col)
        .map(|cell| UnicodeWidthStr::width(cell.text.as_str()).max(1))
        .unwrap_or(1)
}

fn nonzero_size(size: PhysicalSize<u32>) -> PhysicalSize<u32> {
    PhysicalSize::new(size.width.max(1), size.height.max(1))
}

fn composite_alpha_mode(
    capabilities: &wgpu::SurfaceCapabilities,
    background_opacity: f32,
) -> CompositeAlphaMode {
    if background_opacity >= 1.0 {
        return CompositeAlphaMode::Opaque;
    }

    for candidate in [
        CompositeAlphaMode::PreMultiplied,
        CompositeAlphaMode::PostMultiplied,
        CompositeAlphaMode::Inherit,
        CompositeAlphaMode::Auto,
    ] {
        if capabilities.alpha_modes.contains(&candidate) {
            return candidate;
        }
    }

    CompositeAlphaMode::Opaque
}

#[derive(Debug, Clone, Copy)]
struct VisiblePreedit<'a> {
    text: &'a str,
    width: usize,
    start_column: usize,
    selection_start: usize,
    selection_end: usize,
    clipped_start: bool,
    clipped_end: bool,
}

fn ime_available_columns(grid: &GridState) -> usize {
    grid.width().saturating_sub(grid.cursor().col).max(1)
}

fn visible_preedit<'a>(
    text: &'a str,
    cursor: Option<(usize, usize)>,
    available_columns: usize,
) -> VisiblePreedit<'a> {
    let total_width = UnicodeWidthStr::width(text);
    let (selection_start, selection_end) = cursor
        .map(|(start, end)| (byte_column(text, start), byte_column(text, end)))
        .unwrap_or((total_width, total_width));
    let start_column = preedit_window_start(
        selection_start,
        selection_end,
        total_width,
        available_columns,
    );
    let end_column = start_column
        .saturating_add(available_columns)
        .min(total_width);
    let start_byte = byte_index_for_column(text, start_column);
    let end_byte = byte_index_for_column(text, end_column);
    VisiblePreedit {
        text: &text[start_byte..end_byte],
        width: end_column.saturating_sub(start_column),
        start_column,
        selection_start,
        selection_end,
        clipped_start: start_column > 0,
        clipped_end: end_column < total_width,
    }
}

fn preedit_window_start(
    selection_start: usize,
    selection_end: usize,
    total_width: usize,
    available_columns: usize,
) -> usize {
    if total_width <= available_columns {
        return 0;
    }

    let selection_min = selection_start.min(selection_end);
    let selection_max = selection_start.max(selection_end);
    let selection_width = selection_max.saturating_sub(selection_min);

    if selection_width >= available_columns {
        return selection_min.min(total_width.saturating_sub(available_columns));
    }

    let mut start = selection_max.saturating_sub(available_columns);
    if selection_min < start {
        start = selection_min;
    }
    start.min(total_width.saturating_sub(available_columns))
}

fn font_role_for_text(
    text: &str,
    uses_unified_font: bool,
    has_icon_font: bool,
    has_japanese_font: bool,
) -> TextFontRole {
    if uses_unified_font {
        return TextFontRole::Primary;
    }
    if has_icon_font && contains_private_use(text) {
        return TextFontRole::IconFallback;
    }
    if has_japanese_font && contains_japanese(text) {
        return TextFontRole::JapaneseFallback;
    }
    TextFontRole::Primary
}

fn font_family_for_role<'a>(
    role: TextFontRole,
    primary: Option<&'a str>,
    japanese: Option<&'a str>,
    icon: Option<&'a str>,
) -> Family<'a> {
    match role {
        TextFontRole::Primary => primary.map(Family::Name).unwrap_or(Family::Monospace),
        TextFontRole::JapaneseFallback => japanese
            .or(primary)
            .map(Family::Name)
            .unwrap_or(Family::Monospace),
        TextFontRole::IconFallback => icon
            .or(primary)
            .map(Family::Name)
            .unwrap_or(Family::Monospace),
    }
}

fn snap_to_device_pixel(value: f32, scale: f32) -> f32 {
    let scale = scale.max(1.0);
    (value * scale).round() / scale
}

fn snap_line_thickness(scale: f32) -> f32 {
    (1.0 / scale.max(1.0)).max(1.0 / scale.max(1.0))
}

fn undercurl_segments(
    x: f32,
    y: f32,
    cell_width: f32,
    cell_height: f32,
    scale: f32,
) -> Vec<(f32, f32, f32, f32)> {
    let thickness = snap_line_thickness(scale);
    let segment_width = (cell_width / 4.0).max(thickness);
    let baseline = snap_to_device_pixel(y + cell_height - scale.max(1.0) * 2.0 - thickness, scale);
    let amplitude = (cell_height * 0.12).max(thickness);
    let mut segments = Vec::new();
    let mut current_x = x;
    let end_x = x + cell_width;
    let mut phase = 0usize;
    while current_x < end_x {
        let width = (end_x - current_x).min(segment_width);
        let offset = match phase % 4 {
            0 => amplitude,
            1 => 0.0,
            2 => -amplitude,
            _ => 0.0,
        };
        let segment_y =
            snap_to_device_pixel(baseline + offset, scale).clamp(y, y + cell_height - thickness);
        segments.push((
            snap_to_device_pixel(current_x, scale),
            segment_y,
            width,
            thickness,
        ));
        current_x += width;
        phase += 1;
    }
    segments
}

fn byte_column(text: &str, byte_index: usize) -> usize {
    text.get(..byte_index.min(text.len()))
        .map(UnicodeWidthStr::width)
        .unwrap_or_else(|| {
            text.char_indices()
                .take_while(|(index, _)| *index < byte_index)
                .map(|(_, character)| character.width().unwrap_or(0))
                .sum()
        })
}

fn byte_index_for_column(text: &str, target_column: usize) -> usize {
    if target_column == 0 {
        return 0;
    }

    let mut consumed_columns = 0;
    for (index, character) in text.char_indices() {
        let width = character.width().unwrap_or(0);
        if consumed_columns >= target_column {
            return index;
        }
        consumed_columns += width;
        if consumed_columns >= target_column {
            return index + character.len_utf8();
        }
    }
    text.len()
}

fn glyph_positions_are_safe(glyph: &PreparedGlyph) -> bool {
    const MIN_SAFE: f32 = i32::MIN as f32 + 1024.0;
    const MAX_SAFE: f32 = i32::MAX as f32 - 1024.0;

    glyph.buffer.layout_runs().all(|run| {
        run.glyphs.iter().all(|layout| {
            let x = (layout.x + layout.font_size * layout.x_offset) + glyph.left;
            let y = (layout.y - layout.font_size * layout.y_offset) + glyph.top;
            x.is_finite()
                && y.is_finite()
                && (MIN_SAFE..=MAX_SAFE).contains(&x)
                && (MIN_SAFE..=MAX_SAFE).contains(&y)
        })
    })
}

fn find_font_family(font_system: &mut FontSystem, requested: &str) -> Option<String> {
    font_system.db_mut().faces().find_map(|face| {
        face.families
            .iter()
            .map(|(name, _)| name)
            .find(|name| name.eq_ignore_ascii_case(requested.trim()))
            .cloned()
    })
}

fn find_japanese_font(font_system: &mut FontSystem) -> Option<String> {
    const CANDIDATES: &[&str] = &[
        "Hiragino Sans",
        "Hiragino Kaku Gothic ProN",
        "Hiragino Kaku Gothic Pro",
        "Yu Gothic UI",
        "Yu Gothic",
        "YuGothic",
        "Meiryo",
        "MS Gothic",
        "Noto Sans CJK JP",
        "Noto Sans JP",
        "Noto Sans CJK",
        "Osaka",
    ];

    let database = font_system.db_mut();
    for candidate in CANDIDATES {
        if let Some(name) = database.faces().find_map(|face| {
            face.families
                .iter()
                .map(|(name, _)| name)
                .find(|name| name.eq_ignore_ascii_case(candidate))
                .cloned()
        }) {
            return Some(name);
        }
    }
    None
}

fn find_icon_font(font_system: &mut FontSystem) -> Option<String> {
    const CANDIDATES: &[&str] = &[
        "HackGen Console NF",
        "HackGen35 Console NF",
        "Symbols Nerd Font Mono",
        "Symbols Nerd Font",
        "CaskaydiaCove Nerd Font Mono",
        "JetBrainsMono Nerd Font Mono",
        "Hack Nerd Font Mono",
        "FiraCode Nerd Font Mono",
        "MesloLGM Nerd Font Mono",
    ];

    let database = font_system.db_mut();
    for candidate in CANDIDATES {
        if let Some(name) = database.faces().find_map(|face| {
            face.families
                .iter()
                .map(|(name, _)| name)
                .find(|name| name.eq_ignore_ascii_case(candidate))
                .cloned()
        }) {
            return Some(name);
        }
    }
    database.faces().find_map(|face| {
        face.families
            .iter()
            .map(|(name, _)| name)
            .find(|name| is_icon_font_family(name))
            .cloned()
    })
}

fn is_icon_font_family(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("nerd font") || (name.starts_with("hackgen") && name.ends_with(" nf"))
}

fn is_unified_font_family(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("hackgen") && name.contains(" nf")
}

fn contains_japanese(text: &str) -> bool {
    text.chars().any(|character| {
        matches!(
            character as u32,
            0x3000..=0x30ff
                | 0x31f0..=0x31ff
                | 0x3400..=0x4dbf
                | 0x4e00..=0x9fff
                | 0xf900..=0xfaff
                | 0xff00..=0xffef
        )
    })
}

fn contains_private_use(text: &str) -> bool {
    text.chars().any(|character| {
        matches!(
            character as u32,
            0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{
        TextFontRole, byte_column, byte_index_for_column, contains_japanese, contains_private_use,
        cursor_cell_columns, find_font_family, find_icon_font, find_japanese_font,
        font_role_for_text, is_icon_font_family, is_unified_font_family, linear_color,
        preedit_window_start, snap_line_thickness, snap_to_device_pixel, srgb_to_linear,
        undercurl_segments, visible_preedit,
    };
    use crate::nvim::redraw::{GridCell, RedrawEvent};
    use crate::ui::grid::GridState;
    use glyphon::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping};

    #[test]
    fn converts_ime_byte_offsets_to_grid_columns() {
        assert_eq!(byte_column("a日本", 1), 1);
        assert_eq!(byte_column("a日本", 4), 3);
        assert_eq!(byte_column("a日本", 7), 5);
    }

    #[test]
    fn converts_grid_columns_back_to_byte_offsets() {
        assert_eq!(byte_index_for_column("a日本", 0), 0);
        assert_eq!(byte_index_for_column("a日本", 1), 1);
        assert_eq!(byte_index_for_column("a日本", 3), 4);
        assert_eq!(byte_index_for_column("a日本", 5), 7);
    }

    #[test]
    fn keeps_the_preedit_selection_visible_when_text_is_long() {
        let visible = visible_preedit("かな漢字まじりの長い変換テキスト", Some((24, 24)), 6);
        assert!(visible.width <= 6);
        assert!(visible.selection_end >= visible.start_column);
        assert!(visible.selection_end <= visible.start_column + 6);
        assert!(visible.clipped_start || visible.clipped_end);
    }

    #[test]
    fn prefers_the_selection_start_when_selection_is_wider_than_view() {
        let start = preedit_window_start(3, 12, 20, 5);
        assert_eq!(start, 3);
    }

    #[test]
    fn reports_when_preedit_is_clipped_on_each_side() {
        let left = visible_preedit("abcdef", Some((0, 0)), 3);
        assert!(!left.clipped_start);
        assert!(left.clipped_end);

        let right = visible_preedit("abcdef", Some((6, 6)), 3);
        assert!(right.clipped_start);
        assert!(!right.clipped_end);
    }

    #[test]
    fn builds_undercurl_segments_within_the_cell_width() {
        let segments = undercurl_segments(10.0, 20.0, 12.0, 18.0, 2.0);
        assert!(!segments.is_empty());
        let covered_width: f32 = segments.iter().map(|(_, _, width, _)| width).sum();
        assert!((covered_width - 12.0).abs() < 0.001);
        assert!(segments.iter().all(|(x, y, width, height)| {
            *x >= 10.0
                && *x + *width <= 22.001
                && *y >= 20.0
                && *y + *height <= 38.0
                && *height >= 0.5
        }));
    }

    #[test]
    fn snaps_decoration_positions_to_device_pixels_for_hidpi() {
        assert_eq!(snap_to_device_pixel(10.24, 2.0), 10.0);
        assert_eq!(snap_to_device_pixel(10.26, 2.0), 10.5);
        assert_eq!(snap_line_thickness(2.0), 0.5);
    }

    #[test]
    fn prefers_japanese_fallback_only_for_japanese_text() {
        assert_eq!(
            font_role_for_text("日本語", false, true, true),
            TextFontRole::JapaneseFallback
        );
        assert_eq!(
            font_role_for_text("\u{f07c}", false, true, true),
            TextFontRole::IconFallback
        );
        assert_eq!(
            font_role_for_text("plain ASCII", false, true, true),
            TextFontRole::Primary
        );
        assert_eq!(
            font_role_for_text("日本語", true, true, true),
            TextFontRole::Primary
        );
    }

    #[test]
    fn expands_cursor_to_cover_a_wide_character() {
        let mut grid = GridState::default();
        grid.apply(&RedrawEvent::GridResize {
            grid: 1,
            width: 3,
            height: 1,
        });
        grid.apply(&RedrawEvent::GridLine {
            grid: 1,
            row: 0,
            col_start: 0,
            cells: vec![GridCell {
                text: "日".into(),
                highlight_id: Some(0),
                repeat: 1,
            }],
            wrap: false,
        });
        grid.apply(&RedrawEvent::GridCursorGoto {
            grid: 1,
            row: 0,
            col: 0,
        });

        assert_eq!(cursor_cell_columns(&grid), 2);
    }

    #[test]
    fn recognizes_japanese_text() {
        assert!(contains_japanese("日本語入力"));
        assert!(contains_japanese("ひらがな"));
        assert!(!contains_japanese("plain ASCII"));
    }

    #[test]
    fn recognizes_nerd_font_private_use_characters() {
        assert!(contains_private_use("\u{f07c}"));
        assert!(contains_private_use("\u{e0b0}"));
        assert!(!contains_private_use("日本語 abc ◀"));
    }

    #[test]
    fn recognizes_common_icon_font_family_names() {
        assert!(is_icon_font_family("HackGen Console NF"));
        assert!(is_icon_font_family("JetBrainsMono Nerd Font Mono"));
        assert!(!is_icon_font_family("SF Mono"));
        assert!(is_unified_font_family("HackGen Console NF"));
        assert!(!is_unified_font_family("JetBrainsMono Nerd Font Mono"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn finds_installed_nerd_font_when_available() {
        let mut font_system = FontSystem::new();
        let font = find_icon_font(&mut font_system);
        eprintln!("Icon font: {font:?}");
        let font = font.expect("an installed Nerd Font should be detected");
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(15.0, 22.0));
        buffer.set_text(
            &mut font_system,
            "\u{f07c}\u{e0b0}",
            &Attrs::new().family(Family::Name(&font)),
            Shaping::Basic,
            None,
        );
        buffer.shape_until_scroll(&mut font_system, false);
        assert!(
            buffer
                .layout_runs()
                .flat_map(|run| run.glyphs.iter())
                .all(|glyph| glyph.glyph_id != 0)
        );
    }

    #[test]
    fn converts_srgb_colors_for_the_gpu_surface() {
        assert!((srgb_to_linear(0.5) - 0.214_041_14).abs() < 0.0001);
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert_eq!(srgb_to_linear(1.0), 1.0);
        assert_eq!(linear_color(0xff0000, 0.1)[3], 0.1);
    }

    #[test]
    fn finds_configured_font_case_insensitively() {
        let mut font_system = FontSystem::new();
        let font = find_font_family(&mut font_system, "hackgen console nf");
        #[cfg(target_os = "macos")]
        assert_eq!(font.as_deref(), Some("HackGen Console NF"));
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn finds_a_platform_japanese_font() {
        let mut font_system = FontSystem::new();
        let font = find_japanese_font(&mut font_system);
        eprintln!("Japanese font: {font:?}");
        let font = font.expect("a Japanese system font should be available");
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(15.0, 22.0));
        buffer.set_text(
            &mut font_system,
            "日本語",
            &Attrs::new().family(Family::Name(&font)),
            Shaping::Basic,
            None,
        );
        buffer.shape_until_scroll(&mut font_system, false);
        let glyphs = buffer
            .layout_runs()
            .flat_map(|run| run.glyphs.iter())
            .collect::<Vec<_>>();
        assert!(!glyphs.is_empty());
        assert!(glyphs.iter().all(|glyph| glyph.glyph_id != 0));
    }
}
