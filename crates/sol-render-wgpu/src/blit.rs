//! [`BlitPipeline`]: the render pipeline that copies a finished canvas
//! texture onto a host's presentation surface.
//!
//! It lives here, beside [`crate::BLIT_SHADER`], because the two are one
//! contract: the pipeline's bind-group layout has to match the shader's
//! `@group(0) @binding(0)` texture and `@binding(1)` sampler exactly. When
//! only the shader was shared, each host reimplemented the half that has to
//! agree with it, so a change to the bindings had to be mirrored by hand in
//! crates that never see the shader source.
//!
//! Every host draws the same way — one full-screen triangle sampling the
//! canvas at 1:1 — and differs only in surface format and debug labels, so
//! those are the two parameters.

/// The blit pipeline plus the two objects a host needs to bind a canvas to
/// it: the bind-group layout the shader expects, and a nearest-neighbour
/// sampler.
#[derive(Debug)]
pub struct BlitPipeline {
    /// The pipeline itself.
    pub pipeline: wgpu::RenderPipeline,
    /// The bind-group layout [`BlitPipeline::bind`] fills.
    pub layout: wgpu::BindGroupLayout,
    /// A clamped, nearest-neighbour sampler.
    pub sampler: wgpu::Sampler,
}

impl BlitPipeline {
    /// Builds the pipeline for `target_format`, labelling every object it
    /// creates with `label_prefix` so a host keeps its own names in a
    /// graphics debugger.
    #[must_use]
    pub fn new(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        label_prefix: &str,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&format!("{label_prefix} blit shader")),
            source: wgpu::ShaderSource::Wgsl(crate::BLIT_SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&format!("{label_prefix} blit bindings")),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(&format!("{label_prefix} blit pipeline layout")),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(&format!("{label_prefix} blit pipeline")),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        // Nearest: canvas and surface are the same size, texel for texel.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(&format!("{label_prefix} blit sampler")),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        Self {
            pipeline,
            layout,
            sampler,
        }
    }

    /// Binds a (re)created canvas texture to this pipeline.
    #[must_use]
    pub fn bind(
        &self,
        device: &wgpu::Device,
        canvas: &wgpu::Texture,
        label_prefix: &str,
    ) -> wgpu::BindGroup {
        let view = canvas.create_view(&wgpu::TextureViewDescriptor::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("{label_prefix} blit bind group")),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }
}
