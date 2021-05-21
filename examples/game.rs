use bevy::{asset::LoadState, prelude::*};
use bevy_openal::{efx, Context, GlobalEffects, Listener, OpenAlPlugin, Sound, SoundState};

#[derive(Default)]
struct AssetHandles {
    sounds: Vec<HandleUntyped>,
    loaded: bool,
}

fn setup(
    asset_server: Res<AssetServer>,
    mut handles: ResMut<AssetHandles>,
    context: ResMut<Context>,
    mut global_effects: ResMut<GlobalEffects>,
) {
    handles.sounds = asset_server.load_folder(".").expect("Failed to load sfx");
    if let Ok(mut slot) = context.new_aux_effect_slot() {
        if let Ok(mut reverb) = context.new_effect::<efx::EaxReverbEffect>() {
            reverb.set_preset(&efx::REVERB_PRESET_GENERIC).unwrap();
            slot.set_effect(&reverb).unwrap();
            global_effects.push(slot);
        }
    }
}

fn load_and_create(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut handles: ResMut<AssetHandles>,
) {
    if handles.loaded {
        return;
    }
    handles.loaded = asset_server
        .get_group_load_state(handles.sounds.iter().map(|handle| handle.id))
        == LoadState::Loaded;
    if handles.loaded {
        commands.spawn().insert(Listener).insert(Transform::default);
        let handle = handles.sounds[0].clone();
        let buffer = asset_server.get_handle(handle);
        commands
            .spawn()
            .insert(Transform::from_translation(Vec3::new(15., 0., 0.)))
            .insert(Sound {
                buffer,
                state: SoundState::Playing,
                gain: 0.4,
                looping: true,
                ..Default::default()
            });
    }
}

fn main() {
    App::build()
        .add_plugins(DefaultPlugins)
        .add_system(bevy::input::system::exit_on_esc_system.system())
        .add_plugin(OpenAlPlugin)
        .init_resource::<AssetHandles>()
        .add_startup_system(setup.system())
        .add_system(load_and_create.system())
        .run();
}
