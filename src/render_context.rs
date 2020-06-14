use winit::window::Window;

use crate::utils;

/// A `RenderContext` stores any state that is required for rendering a frame. This may include:
///
/// - camera position
/// - asset handles
/// - gpu buffer handles
/// - cached geometry
/// - render pipeline descriptions
/// - shader modules
/// - bind groups and layouts
///
/// Eventually an additional layer should be introduced to abstract all interfacing with the GPU.
#[allow(dead_code)]
pub struct RenderContext {
    surface: wgpu::Surface,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    next_frame_encoder: wgpu::CommandEncoder,

    amplitude: f64,
    frequency: f32,

    vertex_buf: wgpu::Buffer,
    vertex_buf_len: usize,
    index_buf: wgpu::Buffer,
    index_buf_len: usize,

    vs_module: wgpu::ShaderModule,
    fs_module: wgpu::ShaderModule,

    sc_desc: wgpu::SwapChainDescriptor,
    swap_chain: wgpu::SwapChain,

    texture: wgpu::Texture,
    texture_view: wgpu::TextureView,

    sampler: wgpu::Sampler,

    mx_total: cgmath::Matrix4<f32>,

    uniform_buf: wgpu::Buffer,

    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,

    pipeline_layout: wgpu::PipelineLayout,
    render_pipeline: wgpu::RenderPipeline,

    dirty: bool,
}

impl RenderContext {
    // TODO: `Option` -> `Result`.
    pub async fn create(window: &Window) -> Option<RenderContext> {
        let size = window.inner_size();

        // Create the wgpu surface.
        let surface = wgpu::Surface::create(window);

        // Create the wgpu adapter.
        let adapter = wgpu::Adapter::request(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::Default,
                compatible_surface: Some(&surface),
            },
            wgpu::BackendBit::PRIMARY,
        )
        .await
        .unwrap();

        // Create the device handle and the command queue handle for that device.
        let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
            extensions: wgpu::Extensions {
                anisotropic_filtering: false,
            },
            limits: wgpu::Limits::default(),
        })
        .await;

        // We use the encoder to build commands for the command queue.
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        // Create our initial mesh.
        // These variables control the simplex noise generation of the voxel heightmap.
        let amplitude = 10.0f64;
        let frequency = 6.0f32;

        // Build the mesh; these are heap allocated `Vec`s.
        let (vertex_data, index_data) = utils::create_vertices(amplitude, frequency);

        // Now we write the vertex data to a GPU buffer.
        let vertex_slice: &[u8] = bytemuck::cast_slice(&vertex_data);
        let vertex_buf = device.create_buffer_with_data(
            vertex_slice,
            // We will be reusing this buffer to update the terrain, so it needs to be a `COPY_DST`.
            wgpu::BufferUsage::VERTEX | wgpu::BufferUsage::COPY_DST,
        );
        let vertex_buf_len = vertex_data.len() * (utils::VERTEX_SIZE as usize);

        let index_slice: &[u8] = bytemuck::cast_slice(&index_data);
        let index_buf = device.create_buffer_with_data(
            index_slice,
            // We will be reusing this buffer to update the terrain, so it needs to be a `COPY_DST`.
            wgpu::BufferUsage::INDEX | wgpu::BufferUsage::COPY_DST,
        );
        // We are using u32s for the indicies, so divide the byte count by 4.
        let index_buf_len = index_slice.len() / 4;

        // Load the vertex and fragment shaders.
        let vs = include_bytes!("../shaders/shader.vert.spv");
        let vs_module =
            device.create_shader_module(&wgpu::read_spirv(std::io::Cursor::new(&vs[..])).unwrap());

        let fs = include_bytes!("../shaders/shader.frag.spv");
        let fs_module =
            device.create_shader_module(&wgpu::read_spirv(std::io::Cursor::new(&fs[..])).unwrap());

        // Create our swapchain. The swapchain is an abstraction over a buffered pixel array which corresponds directly
        // to the image which is rendered onto the display.
        let sc_desc = wgpu::SwapChainDescriptor {
            usage: wgpu::TextureUsage::OUTPUT_ATTACHMENT,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::Mailbox,
        };

        let swap_chain = device.create_swap_chain(&surface, &sc_desc);

        // Create our texture and write it into a GPU buffer. Right now the texture is just a white image, but the
        // infrastructure is already in place to make better use of this data.
        let size = 256u32;
        let texels = utils::create_texels(size);
        let texture_extent = wgpu::Extent3d {
            width: size,
            height: size,
            depth: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            size: texture_extent,
            array_layer_count: 1,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsage::SAMPLED | wgpu::TextureUsage::COPY_DST,
            label: None,
        });
        let texture_view = texture.create_default_view();
        // Place the texture data into a temporary copy buffer, and then immediately request a copy of it into a texture
        // buffer on the GPU. We wrap this in a lexical scope to avoid reusing `temp_buf`.
        {
            let temp_buf =
                device.create_buffer_with_data(texels.as_slice(), wgpu::BufferUsage::COPY_SRC);
            encoder.copy_buffer_to_texture(
                wgpu::BufferCopyView {
                    buffer: &temp_buf,
                    offset: 0,
                    bytes_per_row: 4 * size,
                    rows_per_image: 0,
                },
                wgpu::TextureCopyView {
                    texture: &texture,
                    mip_level: 0,
                    array_layer: 0,
                    origin: wgpu::Origin3d::ZERO,
                },
                texture_extent,
            );
        }

        // Create the sampler.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            lod_min_clamp: -100.0,
            lod_max_clamp: 100.0,
            compare: wgpu::CompareFunction::Undefined,
        });

        // Create the camera.
        let mx_total = utils::generate_matrix(sc_desc.width as f32 / sc_desc.height as f32);
        let mx_ref: &[f32; 16] = mx_total.as_ref();

        // Create the GPU buffer where we will store our shader uniforms.
        let uniform_buf = device.create_buffer_with_data(
            bytemuck::cast_slice(mx_ref),
            wgpu::BufferUsage::UNIFORM | wgpu::BufferUsage::COPY_DST,
        );

        // Set up our bind groups; this binds our data to named locations which are referenced in the shaders.
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            bindings: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStage::VERTEX,
                    ty: wgpu::BindingType::UniformBuffer { dynamic: false },
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStage::FRAGMENT,
                    ty: wgpu::BindingType::SampledTexture {
                        multisampled: false,
                        component_type: wgpu::TextureComponentType::Float,
                        dimension: wgpu::TextureViewDimension::D2,
                    },
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStage::FRAGMENT,
                    ty: wgpu::BindingType::Sampler { comparison: false },
                },
            ],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &bind_group_layout,
            bindings: &[
                wgpu::Binding {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer {
                        buffer: &uniform_buf,
                        range: 0..mx_ref.len() as u64,
                    },
                },
                wgpu::Binding {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::Binding {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
            label: None,
        });

        // Set up our central render pipeline.
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            bind_group_layouts: &[&bind_group_layout],
        });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            layout: &pipeline_layout,
            vertex_stage: wgpu::ProgrammableStageDescriptor {
                module: &vs_module,
                entry_point: "main",
            },
            fragment_stage: Some(wgpu::ProgrammableStageDescriptor {
                module: &fs_module,
                entry_point: "main",
            }),
            rasterization_state: Some(wgpu::RasterizationStateDescriptor {
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: wgpu::CullMode::Back,
                depth_bias: 0,
                depth_bias_slope_scale: 0.0,
                depth_bias_clamp: 0.0,
            }),
            primitive_topology: wgpu::PrimitiveTopology::TriangleList,
            color_states: &[wgpu::ColorStateDescriptor {
                format: wgpu::TextureFormat::Bgra8UnormSrgb,
                color_blend: wgpu::BlendDescriptor::REPLACE,
                alpha_blend: wgpu::BlendDescriptor::REPLACE,
                write_mask: wgpu::ColorWrite::ALL,
            }],
            depth_stencil_state: None,
            vertex_state: wgpu::VertexStateDescriptor {
                index_format: wgpu::IndexFormat::Uint32,
                vertex_buffers: &[wgpu::VertexBufferDescriptor {
                    stride: utils::VERTEX_SIZE as wgpu::BufferAddress,
                    step_mode: wgpu::InputStepMode::Vertex,
                    attributes: &[
                    wgpu::VertexAttributeDescriptor {
                        format: wgpu::VertexFormat::Float4,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttributeDescriptor {
                            format: wgpu::VertexFormat::Float2,
                            offset: 4 * 4,
                            shader_location: 1,
                        },
                    ],
                }],
            },

            sample_count: 1,
            sample_mask: !0,
            alpha_to_coverage_enabled: false,
        });

        // Flush the initialization commands on the command queue.
        queue.submit(&[encoder.finish()]);

        let next_frame_encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        Some(Self {
            surface,
            adapter,
            device,
            queue,
            next_frame_encoder,
            amplitude,
            frequency,
            vertex_buf,
            vertex_buf_len,
            index_buf,
            index_buf_len,
            vs_module,
            fs_module,
            sc_desc,
            swap_chain,
            texture,
            texture_view,
            sampler,
            mx_total,
            uniform_buf,
            bind_group_layout,
            bind_group,
            pipeline_layout,
            render_pipeline,
            dirty: false,
        })
    }

    pub fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        self.sc_desc.width = size.width;
        self.sc_desc.height = size.height;
        self.swap_chain = self.device.create_swap_chain(&self.surface, &self.sc_desc);

        self.mx_total = utils::generate_matrix(self.sc_desc.width as f32 / self.sc_desc.height as f32);
        let mx_ref: &[f32; 16] = self.mx_total.as_ref();

        let temp_buf =
            self.device.create_buffer_with_data(bytemuck::cast_slice(mx_ref), wgpu::BufferUsage::COPY_SRC);

        self.next_frame_encoder.copy_buffer_to_buffer(&temp_buf, 0, &self.uniform_buf, 0, 64);
    }

    pub fn render(&mut self) {
        let frame = self.swap_chain.get_next_texture().expect("Timeout when acquiring next swap chain texture.");

        if self.dirty {
            self.regenerate_mesh();
            self.dirty = false;
        }

        // Go ahead and pull out the command encoder we have been using to build up this frame. We set up the next
        // frame's encoder at the same time.
        let mut next_frame_encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        std::mem::swap(&mut self.next_frame_encoder, &mut next_frame_encoder);

        {
            let mut render_pass = next_frame_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[wgpu::RenderPassColorAttachmentDescriptor {
                    attachment: &frame.view,
                    resolve_target: None,
                    load_op: wgpu::LoadOp::Clear,
                    store_op: wgpu::StoreOp::Store,
                    clear_color: wgpu::Color {
                        r: 0.1,
                        g: 0.2,
                        b: 0.3,
                        a: 1.0,
                    },
                }],
                depth_stencil_attachment: None,
            });
            render_pass.set_pipeline(&self.render_pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.set_index_buffer(&self.index_buf, 0, 0);
            render_pass.set_vertex_buffer(0, &self.vertex_buf, 0, 0);
            render_pass.draw_indexed(0..self.index_buf_len as u32, 0, 0..1);
        }

        self.queue.submit(&[next_frame_encoder.finish()]);
    }

    // Expose raw mutation for some of the basic state variables.
    pub fn set_dirty(&mut self) {
        self.dirty = true;
    }
    pub fn amplitude(&self) -> f64 {
        self.amplitude
    }
    pub fn set_amplitude(&mut self, amplitude: f64) {
        self.amplitude = amplitude
    }
    pub fn frequency(&self) -> f32 {
        self.frequency
    }
    pub fn set_frequency(&mut self, frequency: f32) {
        self.frequency = frequency
    }

    // Utility functions that mutate local state.
    fn regenerate_mesh(&mut self) {
        let (vertex_data, index_data) = utils::create_vertices(self.amplitude, self.frequency);
        let temp_v_buf = self.device.create_buffer_with_data(bytemuck::cast_slice(&vertex_data), wgpu::BufferUsage::COPY_SRC);
        let temp_i_buf = self.device.create_buffer_with_data(bytemuck::cast_slice(&index_data), wgpu::BufferUsage::COPY_SRC);
        self.next_frame_encoder.copy_buffer_to_buffer(&temp_v_buf, 0, &self.vertex_buf, 0, (vertex_data.len() * 24) as u64);
        self.next_frame_encoder.copy_buffer_to_buffer(&temp_i_buf, 0, &self.index_buf, 0, index_data.len() as u64);
    }

}
