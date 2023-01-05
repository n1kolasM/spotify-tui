use std::io;

use anyhow::{anyhow, Context, Result};

use crate::config::ClientConfig;
use rspotify::{prelude::*, AuthCodePkceSpotify, Config, Credentials};
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
  mut spotify: AuthCodePkceSpotify,
  client_config: &ClientConfig,
) -> Result<AuthCodePkceSpotify> {
  match spotify.read_token_cache(true).await {
    Ok(Some(token)) => {
      *spotify.get_token().lock().await.unwrap() = Some(token);
      spotify.write_token_cache().await?;
      spotify.auto_reauth().await?;
      Ok(spotify)
    }
    _ => match redirect_uri_web_server(&mut spotify, client_config.get_port()) {
      Ok(url) => {
        let full_url = client_config.get_redirect_uri() + &url;
        let code = spotify
          .parse_response_code(&full_url)
          .with_context(|| "Invalid url received from webserver")?;
        spotify.request_token(&code).await?;
        Ok(spotify)
      }
      Err(err) => {
        println!(
          "Starting webserver failed: {}\nContinuing with manual authentication",
          err
        );
        let url = spotify.get_authorize_url(None)?;
        webbrowser::open(&url)?;
        println!("Enter the URL you were redirected to: ");
        let mut input = String::new();
        match io::stdin().read_line(&mut input) {
          Ok(_) => {
            let code = spotify
              .parse_response_code(&input)
              .with_context(|| "Invalid url received from user")?;
            spotify.request_token(&code).await?;
            Ok(spotify)
          }
          Err(err) => Err(anyhow!(err)),
        }
      }
      .with_context(|| "Failed to fetch new spotify token_info"),
    },
  }
}

pub async fn authorize_spotify(client_config: &ClientConfig) -> Result<AuthCodePkceSpotify> {
  let config_paths = client_config.get_or_build_paths()?;

  // Start authorization with spotify
  let creds = Credentials::new_pkce(&client_config.client_id);
  let oauth = rspotify::OAuth {
    redirect_uri: client_config.get_redirect_uri(),
    scopes: SCOPES.iter().map(|&scope| scope.to_owned()).collect(),
    ..Default::default()
  };
  let spotify_config = Config {
    cache_path: config_paths.token_cache_path,
    token_cached: true,
    token_refreshing: true,
    ..Default::default()
  };
  let spotify = AuthCodePkceSpotify::with_config(creds, oauth, spotify_config);
  get_token_auto(spotify, client_config).await
}

pub fn redirect_uri_web_server(spotify: &mut AuthCodePkceSpotify, port: u16) -> Result<String> {
  let listener = TcpListener::bind(("127.0.0.1", port))?;

  let url = spotify.get_authorize_url(None)?;
  webbrowser::open(&url)?;

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
