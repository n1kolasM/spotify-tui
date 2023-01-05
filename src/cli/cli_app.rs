use std::time::Duration;

use crate::network::{IoEvent, Network};
use crate::user_config::UserConfig;

use super::util::{Flag, Format, FormatType, JumpDirection, Type};

use anyhow::{anyhow, Context, Result};
use rand::{thread_rng, Rng};
use rspotify::model::{
  context::CurrentPlaybackContext, parse_uri, AlbumId, ArtistId, EpisodeId, PlayableItem,
  PlaylistId, ShowId, TrackId,
};
use rspotify::prelude::*;

pub struct CliApp<'a> {
  pub net: Network<'a>,
  pub config: UserConfig,
}

// Non-concurrent functions
// I feel that async in a cli is not working
// I just .await all processes and directly interact
// by calling network.handle_network_event
impl<'a> CliApp<'a> {
  pub fn new(net: Network<'a>, config: UserConfig) -> Self {
    Self { net, config }
  }

  async fn is_a_saved_track<'id>(&mut self, id: TrackId<'id>) -> bool {
    // Update the liked_song_ids_set
    self
      .net
      .handle_network_event(IoEvent::CurrentUserSavedTracksContains(vec![
        id.clone_static()
      ]))
      .await;
    self
      .net
      .app
      .lock()
      .await
      .liked_song_ids_set
      .contains(&id.into_static())
  }

  pub fn format_output(&self, mut format: String, values: Vec<Format>) -> String {
    for val in values {
      format = format.replace(val.get_placeholder(), &val.inner(self.config.clone()));
    }
    // Replace unsupported flags with 'None'
    for p in &["%a", "%b", "%t", "%p", "%h", "%u", "%d", "%v", "%f", "%s"] {
      format = format.replace(p, "None");
    }
    format.trim().to_string()
  }

  // spt playback -t
  pub async fn toggle_playback(&mut self) {
    let context = self.net.app.lock().await.current_playback_context.clone();
    if let Some(c) = context {
      if c.is_playing {
        self.net.handle_network_event(IoEvent::PausePlayback).await;
        return;
      }
    }
    self
      .net
      .handle_network_event(IoEvent::StartPlayback(None, None, None))
      .await;
  }

  // spt pb --share-track (share the current playing song)
  // Basically copy-pasted the 'copy_song_url' function
  pub async fn share_track_or_episode(&mut self) -> Result<String> {
    let app = self.net.app.lock().await;
    if let Some(CurrentPlaybackContext {
      item: Some(item), ..
    }) = &app.current_playback_context
    {
      Ok(item.id().with_context(|| "no id for track")?.url())
    } else {
      Err(anyhow!(
        "failed to generate a shareable url for the current song"
      ))
    }
  }

  // spt pb --share-album (share the current album)
  // Basically copy-pasted the 'copy_album_url' function
  pub async fn share_album_or_show(&mut self) -> Result<String> {
    let app = self.net.app.lock().await;
    if let Some(CurrentPlaybackContext {
      item: Some(item), ..
    }) = &app.current_playback_context
    {
      match item {
        PlayableItem::Track(track) => Ok(
          track
            .album
            .id
            .as_ref()
            .with_context(|| "no id for album")?
            .url(),
        ),
        PlayableItem::Episode(episode) => Ok(episode.show.id.url()),
      }
    } else {
      Err(anyhow!(
        "failed to generate a shareable url for the current song"
      ))
    }
  }

  // spt ... -d ... (specify device to control)
  pub async fn set_device(&mut self, name: String) -> Result<()> {
    // Change the device if specified by user
    let mut app = self.net.app.lock().await;
    let mut device_index = 0;
    if let Some(dp) = &app.devices {
      for (i, d) in dp.iter().enumerate() {
        if d.name == name {
          device_index = i;
          // Save the id of the device
          self
            .net
            .client_config
            .set_device_id(d.id.clone().with_context(|| "no id for device")?)
            .map_err(|_e| anyhow!("failed to use device with name '{}'", d.name))?;
        }
      }
    } else {
      // Error out if no device is available
      return Err(anyhow!("no device available"));
    }
    app.selected_device_index = Some(device_index);
    Ok(())
  }

  // spt query ... --limit LIMIT (set max search limit)
  pub async fn update_query_limits(&mut self, max: String) -> Result<()> {
    let num = max
      .parse::<u32>()
      .map_err(|_e| anyhow!("limit must be between 1 and 50"))?;

    // 50 seems to be the maximum limit
    if num > 50 || num == 0 {
      return Err(anyhow!("limit must be between 1 and 50"));
    };

    self
      .net
      .handle_network_event(IoEvent::UpdateSearchLimits(num, num))
      .await;
    Ok(())
  }

  pub async fn volume(&mut self, vol: String) -> Result<()> {
    let num = vol
      .parse::<u32>()
      .map_err(|_e| anyhow!("volume must be between 0 and 100"))?;

    // Check if it's in range
    if num > 100 {
      return Err(anyhow!("volume must be between 0 and 100"));
    };

    self
      .net
      .handle_network_event(IoEvent::ChangeVolume(num as u8))
      .await;
    Ok(())
  }

  // spt playback --next / --previous
  pub async fn jump(&mut self, d: &JumpDirection) {
    match d {
      JumpDirection::Next => self.net.handle_network_event(IoEvent::NextTrack).await,
      JumpDirection::Previous => self.net.handle_network_event(IoEvent::PreviousTrack).await,
    }
  }

  // spt query -l ...
  pub async fn list(&mut self, item: Type, format: &str) -> String {
    match item {
      Type::Device => {
        if let Some(devices) = &self.net.app.lock().await.devices {
          devices
            .iter()
            .map(|d| {
              self.format_output(
                format.to_string(),
                vec![
                  Format::Device(d.name.clone()),
                  Format::Volume(d.volume_percent.unwrap_or_default()),
                ],
              )
            })
            .collect::<Vec<String>>()
            .join("\n")
        } else {
          "No devices available".to_string()
        }
      }
      Type::Playlist => {
        self.net.handle_network_event(IoEvent::GetPlaylists).await;
        if let Some(playlists) = &self.net.app.lock().await.playlists {
          playlists
            .items
            .iter()
            .map(|p| {
              self.format_output(
                format.to_string(),
                Format::from_type(FormatType::Playlist(Box::new(p.clone()))),
              )
            })
            .collect::<Vec<String>>()
            .join("\n")
        } else {
          "No playlists found".to_string()
        }
      }
      Type::Liked => {
        self
          .net
          .handle_network_event(IoEvent::GetCurrentSavedTracks(None))
          .await;
        let liked_songs = self
          .net
          .app
          .lock()
          .await
          .track_table
          .tracks
          .iter()
          .map(|t| {
            self.format_output(
              format.to_string(),
              Format::from_type(FormatType::Track(Box::new(t.clone()))),
            )
          })
          .collect::<Vec<String>>();
        // Check if there are any liked songs
        if liked_songs.is_empty() {
          "No liked songs found".to_string()
        } else {
          liked_songs.join("\n")
        }
      }
      // Enforced by clap
      _ => unreachable!(),
    }
  }

  // spt playback --transfer DEVICE
  pub async fn transfer_playback(&mut self, device: &str) -> Result<()> {
    // Get the device id by name
    let mut id = String::new();
    if let Some(devices) = &self.net.app.lock().await.devices {
      for d in devices {
        if d.name == device {
          id.push_str(d.id.as_ref().map(|id| id.as_str()).unwrap_or("<unknown>"));
          break;
        }
      }
    };

    if id.is_empty() {
      Err(anyhow!("no device with name '{}'", device))
    } else {
      self
        .net
        .handle_network_event(IoEvent::TransferPlaybackToDevice(id.to_string()))
        .await;
      Ok(())
    }
  }

  pub async fn seek(&mut self, seconds_str: String) -> Result<()> {
    let seconds = match seconds_str.parse::<i32>() {
      Ok(s) => s.unsigned_abs(),
      Err(_) => return Err(anyhow!("failed to convert seconds to i32")),
    };

    let (current_pos, duration) = {
      self
        .net
        .handle_network_event(IoEvent::GetCurrentPlayback)
        .await;
      let app = self.net.app.lock().await;
      if let Some(CurrentPlaybackContext {
        progress: Some(progress),
        item: Some(item),
        ..
      }) = &app.current_playback_context
      {
        let duration = match item {
          PlayableItem::Track(track) => track.duration,
          PlayableItem::Episode(episode) => episode.duration,
        };

        (*progress, duration)
      } else {
        return Err(anyhow!("no context available"));
      }
    };

    let seek_duration = Duration::from_secs(seconds.into());
    // Calculate new positon
    let position_to_seek = if seconds_str.starts_with('+') {
      current_pos + seek_duration
    } else if seconds_str.starts_with('-') {
      // Jump to the beginning if the position_to_seek would be
      // negative, must be checked before the calculation to avoid
      // an 'underflow'
      current_pos.saturating_sub(seek_duration)
    } else {
      // Absolute value of the track
      seek_duration
    };

    // Check if position_to_seek is greater than duration (next track)
    if position_to_seek > duration {
      self.jump(&JumpDirection::Next).await;
    } else {
      // This seeks to a position in the current song
      self
        .net
        .handle_network_event(IoEvent::Seek(duration.as_millis() as u32))
        .await;
    }

    Ok(())
  }

  // spt playback --like / --dislike / --shuffle / --repeat
  pub async fn mark(&mut self, flag: Flag) -> Result<()> {
    let c = {
      let app = self.net.app.lock().await;
      app
        .current_playback_context
        .clone()
        .ok_or_else(|| anyhow!("no context available"))?
    };

    match flag {
      Flag::Like(s) => {
        // Get the id of the current song
        let id = match c.item {
          Some(i) => match i {
            PlayableItem::Track(t) => t.id.ok_or_else(|| anyhow!("item has no id")),
            PlayableItem::Episode(_) => Err(anyhow!("saving episodes not yet implemented")),
          },
          None => Err(anyhow!("no item playing")),
        }?;

        // Want to like but is already liked -> do nothing
        // Want to like and is not liked yet -> like
        if s && !self.is_a_saved_track(id.clone()).await {
          self
            .net
            .handle_network_event(IoEvent::ToggleSaveTrack(id.into_static()))
            .await;
        // Want to dislike but is already disliked -> do nothing
        // Want to dislike and is liked currently -> remove like
        } else if !s && self.is_a_saved_track(id.clone()).await {
          self
            .net
            .handle_network_event(IoEvent::ToggleSaveTrack(id.into_static()))
            .await;
        }
      }
      Flag::Shuffle => {
        self
          .net
          .handle_network_event(IoEvent::Shuffle(c.shuffle_state))
          .await
      }
      Flag::Repeat => {
        self
          .net
          .handle_network_event(IoEvent::Repeat(c.repeat_state))
          .await;
      }
    }

    Ok(())
  }

  // spt playback -s
  pub async fn get_status(&mut self, format: String) -> Result<String> {
    // Update info on current playback
    self
      .net
      .handle_network_event(IoEvent::GetCurrentPlayback)
      .await;
    self
      .net
      .handle_network_event(IoEvent::GetCurrentSavedTracks(None))
      .await;

    let context = self
      .net
      .app
      .lock()
      .await
      .current_playback_context
      .clone()
      .ok_or_else(|| anyhow!("no context available"))?;

    let playing_item = context.item.ok_or_else(|| anyhow!("no track playing"))?;

    let mut hs = match playing_item {
      PlayableItem::Track(track) => {
        let id = track.id.clone().ok_or_else(|| anyhow!("no id for track"))?;
        let mut hs = Format::from_type(FormatType::Track(Box::new(track.clone())));
        if let Some(progress) = context.progress {
          hs.push(Format::Position((progress, track.duration)))
        }
        hs.push(Format::Flags((
          context.repeat_state,
          context.shuffle_state,
          self.is_a_saved_track(id).await,
        )));
        hs
      }
      PlayableItem::Episode(episode) => {
        let mut hs = Format::from_type(FormatType::Episode(Box::new(episode.clone())));
        if let Some(ms) = context.progress {
          hs.push(Format::Position((ms, episode.duration)))
        }
        hs.push(Format::Flags((
          context.repeat_state,
          context.shuffle_state,
          false,
        )));
        hs
      }
    };

    hs.push(Format::Device(context.device.name));
    hs.push(Format::Volume(
      context.device.volume_percent.unwrap_or_default(),
    ));
    hs.push(Format::Playing(context.is_playing));

    Ok(self.format_output(format, hs))
  }

  async fn play_impl(
    &mut self,
    context_id: Option<PlayContextId<'static>>,
    playable_id: Option<PlayableId<'static>>,
    queue: bool,
    random: bool,
    display_name: &str,
  ) -> Result<()> {
    let offset = match &context_id {
      Some(PlayContextId::Playlist(playlist_id)) if random => {
        let playlist = self
          .net
          .spotify
          .playlist(playlist_id.clone(), None, None)
          .await?;
        Some(thread_rng().gen_range(0..playlist.tracks.total) as usize)
      }
      Some(PlayContextId::Album(album_id)) if random => {
        let playlist = self.net.spotify.album(album_id.clone()).await?;
        Some(thread_rng().gen_range(0..playlist.tracks.total) as usize)
      }
      _ => None,
    };
    if queue {
      if let Some(playable_id) = playable_id {
        self
          .net
          .handle_network_event(IoEvent::AddItemToQueue(playable_id))
          .await;
      } else {
        return Err(anyhow!(
          "Cannot queue '{}', try playing without queue",
          display_name
        ));
      }
    } else {
      self
        .net
        .handle_network_event(IoEvent::StartPlayback(
          context_id,
          playable_id.map(|id| vec![id]),
          offset,
        ))
        .await;
    }
    Ok(())
  }

  // spt play -u URI
  pub async fn play_uri(&mut self, uri: String, queue: bool, random: bool) -> Result<()> {
    let (_type, id) = parse_uri(&uri)?;
    let (context_id, playable_id) = match _type {
      rspotify::model::Type::Artist => (Some(ArtistId::from_id(id)?.into_static().into()), None),
      rspotify::model::Type::Album => (Some(AlbumId::from_id(id)?.into_static().into()), None),
      rspotify::model::Type::Track => (None, Some(TrackId::from_id(id)?.into_static().into())),
      rspotify::model::Type::Playlist => {
        (Some(PlaylistId::from_id(id)?.into_static().into()), None)
      }
      rspotify::model::Type::Show => (Some(ShowId::from_id(id)?.into_static().into()), None),
      rspotify::model::Type::Episode => (None, Some(EpisodeId::from_id(id)?.into_static().into())),
      _ => {
        return Err(anyhow!("Cannot play '{}'", uri));
      }
    };
    self
      .play_impl(context_id, playable_id, queue, random, &uri)
      .await
  }

  // spt play -n NAME ...
  pub async fn play(&mut self, name: String, item: Type, queue: bool, random: bool) -> Result<()> {
    self
      .net
      .handle_network_event(IoEvent::GetSearchResults(name.clone(), None))
      .await;
    // Get the uri of the first found
    // item + the offset or return an error message
    let (context_id, playable_id) = {
      let results = &self.net.app.lock().await.search_results;
      match item {
        Type::Track => {
          if let Some(r) = &results.tracks {
            let track = &r.items[0];
            if let Some(track_id) = &track.id {
              (None, Some(track_id.clone_static().into()))
            } else {
              return Err(anyhow!("track {} has no id", track.name));
            }
          } else {
            return Err(anyhow!("no tracks with name '{}'", name));
          }
        }
        Type::Album => {
          if let Some(r) = &results.albums {
            let album = &r.items[0];
            if let Some(album_id) = &album.id {
              (Some(album_id.clone_static().into()), None)
            } else {
              return Err(anyhow!("album {} has no uri", album.name));
            }
          } else {
            return Err(anyhow!("no albums with name '{}'", name));
          }
        }
        Type::Artist => {
          if let Some(r) = &results.artists {
            (Some(r.items[0].id.clone_static().into()), None)
          } else {
            return Err(anyhow!("no artists with name '{}'", name));
          }
        }
        Type::Show => {
          if let Some(r) = &results.shows {
            (Some(r.items[0].id.clone_static().into()), None)
          } else {
            return Err(anyhow!("no shows with name '{}'", name));
          }
        }
        Type::Playlist => {
          if let Some(r) = &results.playlists {
            let p = &r.items[0];
            // For a random song, create a random offset
            (Some(p.id.clone_static().into()), None)
          } else {
            return Err(anyhow!("no playlists with name '{}'", name));
          }
        }
        _ => unreachable!(),
      }
    };

    // Play or queue the uri
    self
      .play_impl(context_id, playable_id, queue, random, &name)
      .await
  }

  // spt query -s SEARCH ...
  pub async fn query(&mut self, search: String, format: String, item: Type) -> String {
    self
      .net
      .handle_network_event(IoEvent::GetSearchResults(search.clone(), None))
      .await;

    let app = self.net.app.lock().await;
    match item {
      Type::Playlist => {
        if let Some(results) = &app.search_results.playlists {
          results
            .items
            .iter()
            .map(|r| {
              self.format_output(
                format.clone(),
                Format::from_type(FormatType::Playlist(Box::new(r.clone()))),
              )
            })
            .collect::<Vec<String>>()
            .join("\n")
        } else {
          format!("no playlists with name '{}'", search)
        }
      }
      Type::Track => {
        if let Some(results) = &app.search_results.tracks {
          results
            .items
            .iter()
            .map(|r| {
              self.format_output(
                format.clone(),
                Format::from_type(FormatType::Track(Box::new(r.clone()))),
              )
            })
            .collect::<Vec<String>>()
            .join("\n")
        } else {
          format!("no tracks with name '{}'", search)
        }
      }
      Type::Artist => {
        if let Some(results) = &app.search_results.artists {
          results
            .items
            .iter()
            .map(|r| {
              self.format_output(
                format.clone(),
                Format::from_type(FormatType::Artist(Box::new(r.clone()))),
              )
            })
            .collect::<Vec<String>>()
            .join("\n")
        } else {
          format!("no artists with name '{}'", search)
        }
      }
      Type::Show => {
        if let Some(results) = &app.search_results.shows {
          results
            .items
            .iter()
            .map(|r| {
              self.format_output(
                format.clone(),
                Format::from_type(FormatType::Show(Box::new(r.clone()))),
              )
            })
            .collect::<Vec<String>>()
            .join("\n")
        } else {
          format!("no shows with name '{}'", search)
        }
      }
      Type::Album => {
        if let Some(results) = &app.search_results.albums {
          results
            .items
            .iter()
            .map(|r| {
              self.format_output(
                format.clone(),
                Format::from_type(FormatType::Album(Box::new(r.clone()))),
              )
            })
            .collect::<Vec<String>>()
            .join("\n")
        } else {
          format!("no albums with name '{}'", search)
        }
      }
      // Enforced by clap
      _ => unreachable!(),
    }
  }
}
