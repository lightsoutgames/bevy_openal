use std::{
    collections::HashMap,
    io::Cursor,
    sync::{Arc, Mutex},
};

pub use alto::{efx, Context, Device, Source};
use alto::{efx::AuxEffectSlot, Alto, ContextAttrs, Mono, SourceState, StaticSource, Stereo};
use bevy::{
    asset::{AssetLoader, HandleId, LoadContext, LoadedAsset},
    prelude::*,
    reflect::TypeUuid,
    transform::TransformSystem,
    utils::BoxedFuture,
};
use derive_more::{Deref, DerefMut};
use lewton::inside_ogg::OggStreamReader;
use minimp3::{Decoder, Error};

#[derive(Clone, Debug, TypeUuid)]
#[uuid = "aa22d11e-3bed-11eb-8708-00155dea3db9"]
pub struct Buffer {
    samples: Vec<i16>,
    sample_rate: i32,
    channels: u16,
}

#[derive(Clone, Copy, Debug, Default)]
struct BufferAssetLoader;

impl AssetLoader for BufferAssetLoader {
    fn load<'a>(
        &'a self,
        bytes: &'a [u8],
        load_context: &'a mut LoadContext,
    ) -> BoxedFuture<'a, Result<(), anyhow::Error>> {
        Box::pin(async move {
            let cursor = Cursor::new(bytes.to_vec());
            let buffer: Option<Buffer> =
                match load_context.path().extension().unwrap().to_str().unwrap() {
                    "flac" => {
                        let reader = claxon::FlacReader::new(cursor);
                        if let Ok(mut reader) = reader {
                            let mut samples: Vec<i16> = vec![];
                            for sample in reader.samples().flatten() {
                                samples.push(sample as i16);
                            }
                            let info = reader.streaminfo();
                            Some(Buffer {
                                samples,
                                sample_rate: info.sample_rate as i32,
                                channels: info.channels as u16,
                            })
                        } else {
                            None
                        }
                    }
                    "ogg" => {
                        let mut stream = OggStreamReader::new(cursor)?;
                        let mut samples: Vec<i16> = vec![];
                        while let Some(pck_samples) = &mut stream.read_dec_packet_itl()? {
                            samples.append(pck_samples);
                        }
                        Some(Buffer {
                            samples,
                            channels: stream.ident_hdr.audio_channels as u16,
                            sample_rate: stream.ident_hdr.audio_sample_rate as i32,
                        })
                    }
                    "mp3" => {
                        let mut decoder = Decoder::new(cursor);
                        let mut samples: Vec<i16> = vec![];
                        let mut channels = 0_u16;
                        let mut sample_rate = 0;
                        let mut succeeded = true;
                        loop {
                            match decoder.next_frame() {
                                Ok(mut frame) => {
                                    samples.append(&mut frame.data);
                                    channels = frame.channels as u16;
                                    sample_rate = frame.sample_rate;
                                }
                                Err(Error::Eof) => break,
                                Err(_) => {
                                    succeeded = false;
                                    break;
                                }
                            };
                        }
                        if succeeded {
                            Some(Buffer {
                                samples,
                                channels,
                                sample_rate,
                            })
                        } else {
                            None
                        }
                    }
                    "wav" => {
                        let reader = hound::WavReader::new(cursor);
                        if let Ok(mut reader) = reader {
                            let mut samples: Vec<i16> = vec![];
                            for sample in reader.samples::<i16>().flatten() {
                                samples.push(sample);
                            }
                            Some(Buffer {
                                samples,
                                sample_rate: reader.spec().sample_rate as i32,
                                channels: reader.spec().channels,
                            })
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
            if let Some(buffer) = buffer {
                load_context.set_default_asset(LoadedAsset::new(buffer));
            }
            Ok(())
        })
    }

    fn extensions(&self) -> &[&str] {
        &["flac", "ogg", "mp3", "wav"]
    }
}

// TODO: Make non-public when we have multi-stage asset loading.
#[derive(Default)]
pub struct Buffers(pub HashMap<HandleId, Arc<alto::Buffer>>);

fn buffer_creation(
    context: Res<Context>,
    mut buffers: ResMut<Buffers>,
    mut events: EventReader<AssetEvent<Buffer>>,
    assets: Res<Assets<Buffer>>,
) {
    for event in events.iter() {
        match event {
            AssetEvent::Created { handle } => {
                if let Some(buffer) = assets.get(handle) {
                    let buffer = match buffer.channels {
                        1 => {
                            context.new_buffer::<Mono<i16>, _>(&buffer.samples, buffer.sample_rate)
                        }
                        2 => context
                            .new_buffer::<Stereo<i16>, _>(&buffer.samples, buffer.sample_rate),
                        _ => {
                            panic!("Unsupported channel count");
                        }
                    };
                    if let Ok(buffer) = buffer {
                        buffers.0.insert(handle.id, Arc::new(buffer));
                    }
                }
            }
            AssetEvent::Modified { handle: _ } => {}
            AssetEvent::Removed { handle } => {
                buffers.0.remove(&handle.id);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub enum SoundState {
    Stopped,
    Playing,
    Paused,
}

impl Default for SoundState {
    fn default() -> Self {
        SoundState::Stopped
    }
}

#[derive(Component, Clone, Reflect)]
#[reflect(Component)]
pub struct Sound {
    pub buffer: Handle<Buffer>,
    pub state: SoundState,
    pub gain: f32,
    pub pitch: f32,
    pub looping: bool,
    pub reference_distance: f32,
    pub max_distance: f32,
    pub rolloff_factor: f32,
    pub radius: f32,
    pub bypass_global_effects: bool,
    #[reflect(ignore)]
    pub source: Option<Arc<Mutex<StaticSource>>>,
}

impl Default for Sound {
    fn default() -> Self {
        Self {
            buffer: Default::default(),
            state: Default::default(),
            gain: 1.,
            looping: false,
            pitch: 1.,
            reference_distance: 1.,
            max_distance: f32::MAX,
            rolloff_factor: 1.,
            radius: 0.,
            bypass_global_effects: false,
            source: None,
        }
    }
}

impl Sound {
    pub fn stop(&mut self) {
        if let Some(source) = self.source.as_mut() {
            let mut source = source.lock().unwrap();
            source.stop();
        }
        self.state = SoundState::Stopped;
        self.source = None;
    }

    pub fn play(&mut self) {
        if let Some(source) = self.source.as_mut() {
            let mut source = source.lock().unwrap();
            source.play();
        }
        self.state = SoundState::Playing;
    }

    pub fn pause(&mut self) {
        if let Some(source) = self.source.as_mut() {
            let mut source = source.lock().unwrap();
            source.pause();
        }
        self.state = SoundState::Paused;
    }
}

#[derive(Component, Clone, Copy, Debug, Default, Reflect)]
#[reflect(Component)]
pub struct Listener;

#[derive(Default, Deref, DerefMut)]
pub struct GlobalEffects(Vec<AuxEffectSlot>);

fn update_listener(
    context: ResMut<Context>,
    listener: Query<(Option<&Transform>, Option<&GlobalTransform>), With<Listener>>,
) {
    if let Ok((transform, global_transform)) = listener.get_single() {
        let transform: Option<Transform> = global_transform
            .map(|v| {
                let transform: Transform = (*v).into();
                transform
            })
            .or_else(|| transform.cloned());
        if let Some(transform) = transform {
            let look = transform.local_x();
            let up = transform.local_z();
            if let Err(e) = context.set_position([
                transform.translation.x,
                transform.translation.y,
                transform.translation.z,
            ]) {
                error!("Error setting listener position: {:?}", e);
            }
            if let Err(e) = context.set_orientation(([look.x, look.y, look.z], [up.x, up.y, up.z]))
            {
                error!("Error setting listener orientation: {:?}", e);
            }
        } else {
            context.set_position([0., 0., 0.]).ok();
            context.set_orientation(([0., 0., 1.], [0., 1., 0.])).ok();
        }
    } else {
        context.set_position([0., 0., 0.]).ok();
        context.set_orientation(([0., 0., 1.], [0., 1., 0.])).ok();
    }
}

fn update_source_properties(
    context: Res<Context>,
    buffers: Res<Buffers>,
    mut global_effects: ResMut<GlobalEffects>,
    mut query: Query<(&mut Sound, Option<&Transform>, Option<&GlobalTransform>)>,
) {
    for (mut sound, transform, global_transform) in query.iter_mut() {
        let Sound {
            gain,
            pitch,
            looping,
            reference_distance,
            max_distance,
            rolloff_factor,
            radius,
            bypass_global_effects,
            state,
            ..
        } = *sound;
        if state != SoundState::Stopped {
            let mut swap_buffers = false;
            if let Some(source) = &sound.source {
                let source = source.lock().unwrap();
                if let Some(source_buffer) = source.buffer() {
                    if let Some(sound_buffer) = buffers.0.get(&sound.buffer.id) {
                        if source_buffer.as_raw() != sound_buffer.as_raw() {
                            swap_buffers = true;
                        }
                    }
                }
            }
            if swap_buffers {
                sound.source = None;
            }
            if sound.source.is_none() {
                if let Ok(mut source) = context.new_static_source() {
                    if let Some(buffer) = buffers.0.get(&sound.buffer.id) {
                        source.set_buffer(buffer.clone()).unwrap();
                    }
                    sound.source = Some(Arc::new(Mutex::new(source)));
                }
            }
            if let Some(source) = sound.source.as_mut() {
                let mut source = source.lock().unwrap();
                let translation = global_transform
                    .map(|v| v.translation)
                    .or_else(|| transform.map(|v| v.translation));
                if let Some(translation) = translation {
                    source.set_relative(false);
                    source
                        .set_position([translation.x, translation.y, translation.z])
                        .ok();
                } else {
                    source.set_relative(true);
                    source.set_position([0., 0., 0.]).ok();
                }
                source.set_gain(gain).ok();
                source.set_pitch(pitch).ok();
                source.set_looping(looping);
                source.set_reference_distance(reference_distance).ok();
                source.set_max_distance(max_distance).ok();
                source.set_rolloff_factor(rolloff_factor).ok();
                source.set_radius(radius).ok();
                if !bypass_global_effects {
                    for (send, effect) in global_effects.iter_mut().enumerate() {
                        source.set_aux_send(send as i32, effect).ok();
                    }
                }
            }
        }
    }
}

fn update_source_state(mut query: Query<&mut Sound>) {
    for mut sound in query.iter_mut() {
        let mut clear = false;
        match &sound.state {
            SoundState::Stopped => {
                if let Some(source) = sound.source.as_mut() {
                    let mut source = source.lock().unwrap();
                    source.stop();
                }
                sound.source = None;
            }
            SoundState::Playing => {
                if let Some(source) = sound.source.as_mut() {
                    let mut source = source.lock().unwrap();
                    if !vec![
                        SourceState::Initial,
                        SourceState::Playing,
                        SourceState::Paused,
                    ]
                    .contains(&source.state())
                    {
                        clear = true;
                    } else if source.state() != SourceState::Playing {
                        source.play();
                    }
                }
            }
            SoundState::Paused => {
                if let Some(source) = sound.source.as_mut() {
                    let mut source = source.lock().unwrap();
                    if source.state() != SourceState::Paused {
                        source.pause();
                    }
                }
            }
        }
        if clear {
            sound.source = None;
            sound.state = SoundState::Stopped;
        }
        if let Some(source) = sound.source.clone() {
            let source = source.lock().unwrap();
            sound.state = match &source.state() {
                SourceState::Initial => SoundState::Stopped,
                SourceState::Playing => SoundState::Playing,
                SourceState::Paused => SoundState::Paused,
                SourceState::Stopped => SoundState::Stopped,
                SourceState::Unknown(_) => SoundState::Stopped,
            };
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OpenAlConfig {
    pub soft_hrtf: bool,
}

pub struct OpenAlPlugin;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemLabel)]
pub enum OpenAlSystem {
    UpdateListener,
    UpdateSourceProperties,
    UpdateSourceState,
}

impl Plugin for OpenAlPlugin {
    fn build(&self, app: &mut App) {
        if !app.world.contains_resource::<OpenAlConfig>() {
            app.insert_resource(OpenAlConfig::default());
        }
        let config = *app.world.get_resource::<OpenAlConfig>().unwrap();
        let al = Alto::load_default().expect("Could not load alto");
        let device = al.open(None).expect("Could not open device");
        let mut context_attrs = ContextAttrs::default();
        if config.soft_hrtf {
            context_attrs.soft_hrtf = Some(true);
        }
        let context = device
            .new_context(Some(context_attrs))
            .expect("Could not create context");
        app.add_asset::<Buffer>()
            .init_asset_loader::<BufferAssetLoader>()
            .insert_non_send_resource(device)
            .insert_resource(context)
            .insert_resource(Buffers::default())
            .insert_resource(GlobalEffects::default())
            .register_type::<Listener>()
            .add_system(buffer_creation)
            .add_system_to_stage(
                CoreStage::PostUpdate,
                update_listener
                    .label(OpenAlSystem::UpdateListener)
                    .after(TransformSystem::TransformPropagate)
                    .before(OpenAlSystem::UpdateSourceState),
            )
            .add_system_to_stage(
                CoreStage::PostUpdate,
                update_source_properties
                    .label(OpenAlSystem::UpdateSourceProperties)
                    .after(TransformSystem::TransformPropagate)
                    .before(OpenAlSystem::UpdateSourceState),
            )
            .add_system_to_stage(
                CoreStage::PostUpdate,
                update_source_state.label(OpenAlSystem::UpdateSourceState),
            );
    }
}
