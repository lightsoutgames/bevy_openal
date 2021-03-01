use std::collections::HashMap;
use std::io::Cursor;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

pub use alto::efx;
pub use alto::Context;
pub use alto::Device;
pub use alto::Source;
use alto::{efx::AuxEffectSlot, SourceState};
use alto::{Alto, Mono, StaticSource, Stereo};
use bevy::{
    asset::{AssetLoader, HandleId, LoadContext, LoadedAsset},
    prelude::*,
    reflect::TypeUuid,
    utils::BoxedFuture,
};
use lewton::inside_ogg::OggStreamReader;

#[derive(Clone, Debug, TypeUuid)]
#[uuid = "aa22d11e-3bed-11eb-8708-00155dea3db9"]
pub struct Buffer {
    samples: Vec<i16>,
    sample_rate: i32,
    channels: u16,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BufferAssetLoader;

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
                            for sample in reader.samples() {
                                if let Ok(sample) = sample {
                                    samples.push(sample as i16);
                                }
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
                    "wav" => {
                        let reader = hound::WavReader::new(cursor);
                        if let Ok(mut reader) = reader {
                            let mut samples: Vec<i16> = vec![];
                            for sample in reader.samples::<i16>() {
                                if let Ok(sample) = sample {
                                    samples.push(sample);
                                }
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
        &["flac", "ogg", "wav"]
    }
}

#[derive(Default)]
struct Buffers(HashMap<HandleId, Arc<alto::Buffer>>);

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

#[derive(Reflect)]
pub struct Sound {
    pub buffer: Handle<Buffer>,
    pub state: SoundState,
    pub gain: f32,
    pub looping: bool,
    pub pitch: f32,
    #[reflect(ignore)]
    pub source: Option<StaticSource>,
}

impl Default for Sound {
    fn default() -> Self {
        Self {
            buffer: Default::default(),
            state: Default::default(),
            gain: 1.,
            looping: false,
            pitch: 1.,
            source: None,
        }
    }
}

#[derive(Default)]
pub struct GlobalEffects(Vec<AuxEffectSlot>);

impl Deref for GlobalEffects {
    type Target = Vec<AuxEffectSlot>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for GlobalEffects {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

fn update_source_position(
    source: &mut StaticSource,
    transform: Option<&Transform>,
    global_transform: Option<&GlobalTransform>,
) {
    let translation = global_transform
        .map(|v| v.translation)
        .or_else(|| transform.map(|v| v.translation));
    if let Some(translation) = translation {
        source.set_relative(false);
        source
            .set_position([translation.x, translation.y, translation.z])
            .unwrap();
    } else {
        //println!("No translation: {:?}, {:?}", transform, global_transform);
        source.set_relative(true);
        source.set_position([0., 0., 0.]).unwrap();
    }
}

fn source_update(
    context: Res<Context>,
    buffers: Res<Buffers>,
    mut global_effects: ResMut<GlobalEffects>,
    mut query: Query<(&mut Sound, Option<&Transform>, Option<&GlobalTransform>)>,
) {
    for (mut sound, transform, global_transform) in query.iter_mut() {
        let state = sound.state;
        match state {
            SoundState::Stopped => {
                if let Some(source) = sound.source.as_mut() {
                    source.stop();
                    sound.source = None;
                }
            }
            SoundState::Playing => {
                if sound.source.is_none() {
                    let mut source = context.new_static_source().unwrap();
                    if let Some(buffer) = buffers.0.get(&sound.buffer.id) {
                        source.set_buffer(buffer.clone()).unwrap();
                    }
                    update_source_position(&mut source, transform, global_transform);
                    source.play();
                    sound.source = Some(source);
                }
            }
            SoundState::Paused => {
                if let Some(source) = sound.source.as_mut() {
                    source.pause();
                } else {
                    let mut source = context.new_static_source().unwrap();
                    if let Some(buffer) = buffers.0.get(&sound.buffer.id) {
                        source.set_buffer(buffer.clone()).unwrap();
                    }
                    source.pause();
                    sound.source = Some(source);
                }
            }
        }
        let state = sound.state;
        let gain = sound.gain;
        let looping = sound.looping;
        let pitch = sound.pitch;
        let source_state = if let Some(source) = &sound.source {
            Some(source.state())
        } else {
            None
        };
        if let Some(source_state) = source_state {
            sound.state = match source_state {
                SourceState::Initial => SoundState::Stopped,
                SourceState::Playing => SoundState::Playing,
                SourceState::Paused => SoundState::Paused,
                SourceState::Stopped => SoundState::Stopped,
                SourceState::Unknown(_) => SoundState::Stopped,
            };
        } else {
            sound.state = SoundState::Stopped;
        }
        if let Some(source) = sound.source.as_mut() {
            if state != SoundState::Stopped {
                source.set_gain(gain).unwrap();
                source.set_looping(looping);
                source.set_pitch(pitch).unwrap();
                update_source_position(source, transform, global_transform);
                for (send, effect) in global_effects.iter_mut().enumerate() {
                    source.set_aux_send(send as i32, effect).unwrap();
                }
            } else {
                sound.source = None;
            }
        }
    }
}

impl Sound {
    pub fn stop(&mut self) {
        if let Some(source) = self.source.as_mut() {
            source.stop();
        }
        self.state = SoundState::Stopped;
        self.source = None;
    }

    pub fn play(&mut self) {
        if let Some(source) = self.source.as_mut() {
            source.play();
        }
        self.state = SoundState::Playing;
    }

    pub fn pause(&mut self) {
        if let Some(source) = self.source.as_mut() {
            source.pause();
        }
        self.state = SoundState::Paused;
    }
}

#[derive(Clone, Copy, Debug, Default, Reflect)]
#[reflect(Component)]
pub struct Listener;

fn listener_update(
    context: ResMut<Context>,
    query: Query<(&Listener, Option<&Transform>, Option<&GlobalTransform>)>,
) {
    for (_, transform, global_transform) in query.iter() {
        let transform: Option<Transform> = global_transform
            .map(|v| {
                let transform: Transform = (*v).into();
                transform
            })
            .or_else(|| transform.cloned());
        if let Some(transform) = transform {
            let matrix = transform.compute_matrix().inverse();
            let look = matrix.x_axis;
            let up = matrix.z_axis;
            context
                .set_position([
                    transform.translation.x,
                    transform.translation.y,
                    transform.translation.z,
                ])
                .unwrap();
            context
                .set_orientation(([look.x, look.y, look.z], [up.x, up.y, up.z]))
                .unwrap();
        } else {
            context.set_position([0., 0., 0.]).unwrap();
            context
                .set_orientation(([0., 0., 1.], [0., 1., 0.]))
                .unwrap();
        }
    }
}

pub struct OpenAlPlugin;

impl Plugin for OpenAlPlugin {
    fn build(&self, app: &mut AppBuilder) {
        let al = Alto::load_default().expect("Could not load alto");
        let device = al.open(None).expect("Could not open device");
        let context = device.new_context(None).expect("Could not create context");
        app.add_asset::<Buffer>()
            .init_asset_loader::<BufferAssetLoader>()
            .insert_non_send_resource(device)
            .insert_resource(context)
            .insert_resource(Buffers::default())
            .insert_resource(GlobalEffects::default())
            .register_type::<Listener>()
            .add_system(buffer_creation.system())
            .add_system(source_update.system())
            .add_system(listener_update.system());
    }
}
