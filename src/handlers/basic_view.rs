use crate::{app::App, event::Key, network::IoEvent};
use rspotify::model::{context::CurrentPlaybackContext, PlayableItem};

pub fn handler(key: Key, app: &mut App) {
  if let Key::Char('s') = key {
    if let Some(CurrentPlaybackContext {
      item: Some(item), ..
    }) = &app.current_playback_context
    {
      match item {
        PlayableItem::Track(track) => {
          if let Some(track_id) = &track.id {
            app.dispatch(IoEvent::ToggleSaveTrack(track_id.clone_static()));
          }
        }
        PlayableItem::Episode(_episode) => (),
      };
    };
  }
}
