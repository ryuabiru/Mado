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
    glyphs: Vec<PreparedGlyph>,
    rect_vertices: Vec<RectVertex>,
    rect_vertex_buffer: Option<wgpu::Buffer>,
    rect_vertex_capacity: u64,
    font_family: Option<String>,
    base_font_size: f32,
    japanese_font: Option<String>,
    icon_font: Option<String>,
    scale_factor: f64,
    window: Arc<Window>,
}

impl Renderer {
    pub async fn new(
        window: Arc<Window>,
        event_loop: &ActiveEventLoop,
        font: &FontConfig,
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
            alpha_mode: CompositeAlphaMode::Opaque,
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
            glyphs: Vec::new(),
            rect_vertices: Vec::new(),
            rect_vertex_buffer: None,
            rect_vertex_capacity: 0,
            font_family,
            base_font_size: font.size,
            japanese_font,
            icon_font,
            scale_factor: window.scale_factor(),
            window,
        })
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>, scale_factor: f64) {
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

    pub fn ime_cursor_area(
        &self,
        grid: &GridState,
        preedit: Option<(&str, Option<(usize, usize)>)>,
    ) -> (PhysicalPosition<f64>, PhysicalSize<u32>) {
        let cursor = grid.cursor();
        let preedit_columns = preedit
            .and_then(|(text, cursor)| cursor.and_then(|(start, _)| text.get(..start)))
            .map(UnicodeWidthStr::width)
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
    ) -> Result<()> {
        self.prepare_glyphs(grid, preedit);
        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.config.width,
                height: self.config.height,
            },
        );

        let unsafe_glyphs = self
            .glyphs
            .iter()
            .filter(|glyph| !glyph_positions_are_safe(glyph))
            .count();
        if unsafe_glyphs > 0 {
            tracing::warn!(
                unsafe_glyphs,
                "skipping text with invalid glyph coordinates"
            );
        }
        let text_areas = self
            .glyphs
            .iter()
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
        self.append_rectangle_vertices(grid, preedit, &mut rect_vertices);
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
                        load: LoadOp::Clear(wgpu_color(grid.background())),
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
    ) {
        self.glyphs.clear();
        let metrics = Metrics::new(self.font_size(), self.cell_height());
        let cell_width = self.cell_width();
        let cell_height = self.cell_height();
        let padding = self.padding();
        for row in 0..grid.height() {
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
                let icon_family = self.icon_font.as_deref().map(Family::Name);
                let japanese_family = self.japanese_font.as_deref().map(Family::Name);
                let uses_icon_font =
                    !uses_unified_font && contains_private_use(&cell.text) && icon_family.is_some();
                let uses_japanese_font = !uses_unified_font
                    && !uses_icon_font
                    && contains_japanese(&cell.text)
                    && japanese_family.is_some();
                let family = if uses_unified_font {
                    self.font_family
                        .as_deref()
                        .map(Family::Name)
                        .unwrap_or(Family::Monospace)
                } else if uses_icon_font {
                    icon_family.unwrap_or(Family::Monospace)
                } else if uses_japanese_font {
                    japanese_family.unwrap_or(Family::Monospace)
                } else {
                    self.font_family
                        .as_deref()
                        .map(Family::Name)
                        .unwrap_or(Family::Monospace)
                };
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
                    if uses_unified_font || uses_icon_font || uses_japanese_font {
                        Shaping::Basic
                    } else {
                        Shaping::Advanced
                    },
                    None,
                );
                buffer.shape_until_scroll(&mut self.font_system, false);
                self.glyphs.push(PreparedGlyph {
                    buffer,
                    left: padding + col as f32 * cell_width,
                    top: padding + row as f32 * cell_height,
                    color: glyphon_color(
                        if grid.cursor_style().shape == CursorShape::Block
                            && grid.cursor_is_on_main_grid()
                            && grid.cursor().row == row
                            && grid.cursor().col == col
                        {
                            highlight.background
                        } else {
                            highlight.foreground
                        },
                    ),
                });
            }
        }

        if let Some((text, _)) = preedit.filter(|(text, _)| !text.is_empty()) {
            let mut buffer = Buffer::new(&mut self.font_system, metrics);
            buffer.set_wrap(&mut self.font_system, Wrap::None);
            buffer.set_size(
                &mut self.font_system,
                Some((UnicodeWidthStr::width(text).max(1) as f32 + 1.0) * cell_width),
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
            buffer.set_text(&mut self.font_system, text, &attrs, shaping, None);
            buffer.shape_until_scroll(&mut self.font_system, false);
            let cursor = grid.cursor();
            self.glyphs.push(PreparedGlyph {
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
        vertices: &mut Vec<RectVertex>,
    ) {
        for (index, cell) in grid.cells().iter().enumerate() {
            let highlight = grid.resolve_highlight(cell.highlight_id);
            let row = index / grid.width().max(1);
            let col = index % grid.width().max(1);
            let x = self.padding() + col as f32 * self.cell_width();
            let y = self.padding() + row as f32 * self.cell_height();
            if highlight.background != grid.background() {
                self.push_rect(
                    vertices,
                    x,
                    y,
                    self.cell_width(),
                    self.cell_height(),
                    highlight.background,
                );
            }
            if highlight.underline || highlight.undercurl {
                self.push_rect(
                    vertices,
                    x,
                    y + self.cell_height() - self.scale_factor as f32 * 2.0,
                    self.cell_width(),
                    self.scale_factor as f32,
                    highlight.special,
                );
            }
            if highlight.strikethrough {
                self.push_rect(
                    vertices,
                    x,
                    y + self.cell_height() * 0.55,
                    self.cell_width(),
                    self.scale_factor as f32,
                    highlight.special,
                );
            }
            if highlight.overline {
                self.push_rect(
                    vertices,
                    x,
                    y,
                    self.cell_width(),
                    self.scale_factor as f32,
                    highlight.special,
                );
            }
        }

        if grid.cursor_is_on_main_grid() && grid.cursor().row < grid.height() {
            let cursor = grid.cursor();
            let style = grid.cursor_style();
            let highlight = grid
                .cell(cursor.row, cursor.col)
                .map(|cell| grid.resolve_highlight(cell.highlight_id))
                .unwrap_or_else(|| grid.resolve_highlight(0));
            let color = cursor_color(grid, &style, highlight);
            let percentage = style.cell_percentage.clamp(1, 100) as f32 / 100.0;
            let (x_offset, y_offset, width, height) = match style.shape {
                CursorShape::Block => (0.0, 0.0, self.cell_width(), self.cell_height()),
                CursorShape::Vertical => {
                    (0.0, 0.0, self.cell_width() * percentage, self.cell_height())
                }
                CursorShape::Horizontal => (
                    0.0,
                    self.cell_height() * (1.0 - percentage),
                    self.cell_width(),
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
            let grid_cursor = grid.cursor();
            let width = UnicodeWidthStr::width(text).max(1) as f32 * self.cell_width();
            let x = self.padding() + grid_cursor.col as f32 * self.cell_width();
            let y = self.padding() + grid_cursor.row as f32 * self.cell_height();
            self.push_rect(vertices, x, y, width, self.cell_height(), 0x3b4252);
            if let Some((start, end)) = preedit_cursor {
                let start_col = byte_column(text, start);
                let end_col = byte_column(text, end);
                let selection_start = start_col.min(end_col) as f32 * self.cell_width();
                let selection_width = start_col.abs_diff(end_col) as f32 * self.cell_width();
                if selection_width > 0.0 {
                    self.push_rect(
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
        let left = x / self.config.width as f32 * 2.0 - 1.0;
        let right = (x + width) / self.config.width as f32 * 2.0 - 1.0;
        let top = 1.0 - y / self.config.height as f32 * 2.0;
        let bottom = 1.0 - (y + height) / self.config.height as f32 * 2.0;
        let color = linear_color(color);
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

fn wgpu_color(rgb: u32) -> wgpu::Color {
    wgpu::Color {
        r: srgb_to_linear(((rgb >> 16) & 0xff) as f32 / 255.0) as f64,
        g: srgb_to_linear(((rgb >> 8) & 0xff) as f32 / 255.0) as f64,
        b: srgb_to_linear((rgb & 0xff) as f32 / 255.0) as f64,
        a: 1.0,
    }
}

fn linear_color(rgb: u32) -> [f32; 4] {
    [
        srgb_to_linear(((rgb >> 16) & 0xff) as f32 / 255.0),
        srgb_to_linear(((rgb >> 8) & 0xff) as f32 / 255.0),
        srgb_to_linear((rgb & 0xff) as f32 / 255.0),
        1.0,
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

fn nonzero_size(size: PhysicalSize<u32>) -> PhysicalSize<u32> {
    PhysicalSize::new(size.width.max(1), size.height.max(1))
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
        byte_column, contains_japanese, contains_private_use, find_font_family, find_icon_font,
        find_japanese_font, is_icon_font_family, is_unified_font_family, srgb_to_linear,
    };
    use glyphon::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping};

    #[test]
    fn converts_ime_byte_offsets_to_grid_columns() {
        assert_eq!(byte_column("a日本", 1), 1);
        assert_eq!(byte_column("a日本", 4), 3);
        assert_eq!(byte_column("a日本", 7), 5);
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
