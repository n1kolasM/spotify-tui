use std::io;

use anyhow::{anyhow, Context, Result};

use crate::config::ClientConfig;
use rspotify::{
  oauth2::{SpotifyOAuth, TokenInfo},
  util::{process_token, request_token},
};
use std::{
  io::prelude::*,
  net::{TcpListener, TcpStream},
};

const SCOPES: [&str; 14] = [
  "playlist-read-collaborative",
  "playlist-read-private",
  "playlist-modify-private",
  "playlist-modify-public",
  "user-follow-read",
  "user-follow-modify",
  "user-library-modify",
  "user-library-read",
  "user-modify-playback-state",
  "user-read-currently-playing",
  "user-read-playback-state",
  "user-read-playback-position",
  "user-read-private",
  "user-read-recently-played",
];

/// get token automatically with local webserver
async fn get_token_auto(
  spotify_oauth: &mut SpotifyOAuth,
  client_config: &ClientConfig,
) -> Result<TokenInfo> {
  match spotify_oauth.get_cached_token().await {
    Some(token_info) => Ok(token_info),
    None => match redirect_uri_web_server(spotify_oauth, client_config.get_port()) {
      Ok(mut url) => process_token(spotify_oauth, &mut url)
        .await
        .ok_or_else(|| anyhow!("Failed to process response token")),
      Err(err) => {
        println!(
          "Starting webserver failed: {}\nContinuing with manual authentication",
          err
        );
        request_token(spotify_oauth);
        println!("Enter the URL you were redirected to: ");
        let mut input = String::new();
        match io::stdin().read_line(&mut input) {
          Ok(_) => process_token(spotify_oauth, &mut input)
            .await
            .ok_or_else(|| anyhow!("Failed to process response token")),
          Err(err) => Err(anyhow!(err)),
        }
      }
      .with_context(|| "Failed to fetch new spotify token_info"),
    },
  }
}

pub async fn authorize_spotify(client_config: &ClientConfig) -> Result<(SpotifyOAuth, TokenInfo)> {
  let config_paths = client_config.get_or_build_paths()?;

  // Start authorization with spotify
  let mut oauth = SpotifyOAuth::default()
    .client_id(&client_config.client_id)
    .client_secret(&client_config.client_secret)
    .redirect_uri(&client_config.get_redirect_uri())
    .cache_path(config_paths.token_cache_path)
    .scope(&SCOPES.join(" "))
    .build();
  let token_info = get_token_auto(&mut oauth, client_config).await?;
  Ok((oauth, token_info))
}

pub fn redirect_uri_web_server(spotify_oauth: &mut SpotifyOAuth, port: u16) -> Result<String> {
  let listener = TcpListener::bind(("127.0.0.1", port))?;

  request_token(spotify_oauth);

  for stream in listener.incoming() {
    match stream {
      Ok(stream) => {
        if let Some(url) = handle_connection(stream) {
          return Ok(url);
        }
      }
      Err(e) => {
        println!("Error: {}", e);
      }
    };
  }
  Err(anyhow!("Failed accepting connections"))
}

fn handle_connection(mut stream: TcpStream) -> Option<String> {
  // The request will be quite large (> 512) so just assign plenty just in case
  let mut buffer = [0; 1000];
  let _ = stream.read(&mut buffer).unwrap();

  // convert buffer into string and 'parse' the URL
  match String::from_utf8(buffer.to_vec()) {
    Ok(request) => {
      let split: Vec<&str> = request.split_whitespace().collect();

      if split.len() > 1 {
        respond_with_success(stream);
        return Some(split[1].to_string());
      }

      respond_with_error("Malformed request".to_string(), stream);
    }
    Err(e) => {
      respond_with_error(format!("Invalid UTF-8 sequence: {}", e), stream);
    }
  };

  None
}

fn respond_with_success(mut stream: TcpStream) {
  let contents = include_str!("redirect_uri.html");

  let response = format!("HTTP/1.1 200 OK\r\n\r\n{}", contents);

  stream.write_all(response.as_bytes()).unwrap();
  stream.flush().unwrap();
}

fn respond_with_error(error_message: String, mut stream: TcpStream) {
  println!("Error: {}", error_message);
  let response = format!(
    "HTTP/1.1 400 Bad Request\r\n\r\n400 - Bad Request - {}",
    error_message
  );

  stream.write_all(response.as_bytes()).unwrap();
  stream.flush().unwrap();
}
