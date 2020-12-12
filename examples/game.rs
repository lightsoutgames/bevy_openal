use bevy::{asset::LoadState, prelude::*};
use bevy_openal::{Listener, OpenAlPlugin, Sound, Sounds};

#[derive(Default)]
struct AssetHandles {
    sounds: Vec<HandleUntyped>,
    loaded: bool,
}

fn setup(asset_server: Res<AssetServer>, mut handles: ResMut<AssetHandles>) {
    handles.sounds = asset_server.load_folder(".").expect("Failed to load sfx");
}

fn load_and_create_system(
    commands: &mut Commands,
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
        commands.spawn((Listener::default(), Transform::default));
        let handle = handles.sounds[0].clone();
        let buffer = asset_server.get_handle(handle);
        let mut sounds = Sounds::default();
        sounds.insert(
            "footstep".into(),
            Sound {
                buffer,
                autoplay: true,
                looping: true,
                ..Default::default()
            },
        );
        commands.spawn((Transform::from_translation(Vec3::new(15., 0., 0.)), sounds));
    }
}

fn main() {
    App::build()
        .add_plugins(DefaultPlugins)
        .add_system(bevy::input::system::exit_on_esc_system)
        .add_plugin(OpenAlPlugin)
        .init_resource::<AssetHandles>()
        .add_startup_system(setup)
        .add_system(load_and_create_system)
        .run();
}
