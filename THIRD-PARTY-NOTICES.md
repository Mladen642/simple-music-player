# Third-party notices

## Included in the app

- **Inter** font by Rasmus Andersson: SIL Open Font License 1.1, see
  [assets/Inter-LICENSE.txt](assets/Inter-LICENSE.txt).
- **Phosphor Icons** (through the `egui-phosphor` crate): MIT License.
- **Rust libraries** listed in `Cargo.toml` and `Cargo.lock`, each under its own
  license, mostly MIT or Apache-2.0 (Symphonia, the audio decoder: MPL-2.0).
  `cargo tree` lists them all.

## Downloaded by the user, not included

The download feature uses three programs. If they aren't on the computer already,
the app offers to download them from their official GitHub releases into its data
folder. They are not part of this repository or the AppImage:

- **yt-dlp**: The Unlicense (public domain), https://github.com/yt-dlp/yt-dlp
- **FFmpeg** (builds from https://github.com/yt-dlp/FFmpeg-Builds): GPL v3
- **Deno**: MIT License, https://github.com/denoland/deno

## Online services

The app looks things up on these services, only with song, artist and album names:

- Covers: iTunes Search (Apple), Deezer, MusicBrainz and the Cover Art Archive
- Lyrics: LRCLIB, lrcmux (YouTube Music, Genius, KuGou) and NetEase Cloud Music
  (its public web API, unofficial)
- Albums and genres: Wikipedia

Downloading music is meant for music you're allowed to download. Please respect the
terms of the sites you use and the rights of the artists.
