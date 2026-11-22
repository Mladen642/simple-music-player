# Music Player

A music player for Linux with its own window: your music folder with covers,
favorites, statistics, timed lyrics, an equalizer, even volume between songs,
song editing, tidy file names, duplicates and 13 themes. Written in Rust with egui.

## Running it

Download `MusicPlayer-x86_64.AppImage`, make it executable once (right-click >
Properties > Permissions > "Allow executing as program", or
`chmod +x MusicPlayer-x86_64.AppImage`), then double-click it. Nothing else to
install. It runs on Linux systems from 2022 on (Ubuntu 22.04, Mint 21, Debian 12,
Fedora 36 or newer).

On the first start it asks: **use an existing library**, or **start fresh**. An
existing library is a folder with `library.json` in it: the old (web) player's
folder, or a Music Player data folder from another computer
(`~/.local/share/music-player` there). Hearts, plays, edited names, covers and saved
lyrics come along. Friends who start fresh get your Music folder's songs and an
empty library; the AppImage never contains anyone's data.

Keys: Space plays and pauses, `/` jumps to the search box, G and H move timed
lyrics earlier and later, Escape closes menus. Media keys and the desktop's media
controls work too.

## Where everything is kept

In `~/.local/share/music-player` (another place: start it with
`MUSIC_PLAYER_DATA=/some/folder`). Your music files change only when you ask:
the pencil, Tidy file names, and deleting (which moves them to the Trash).

| File | What's in it |
| --- | --- |
| `library.json` | One record per song: its title, artist, album and genres as the player shows them, the heart, plays, the lyrics source you picked, the names you changed yourself |
| `library.backup.json` | library.json as it was at the last start |
| `settings.json` | Your default music folder |
| `preferences.json` | Theme, volume, sorting, shuffle, Even volume |
| `loudness.json` | How loud each song is (for Even volume), measured once |
| `tags.json` | What each song file says about itself (title, artist, length...), so starting doesn't read every file again. Safe to delete: it's made again |
| `covers/` | Covers found online (songs with a picture inside or next to them don't need one) |
| `lyrics/` | Lyrics you saved, named "Artist - Title.lrc" (timed) or ".txt" |
| `last-rename.json` | The last renames, so "Undo last rename" can put them back |
| `bin/` | yt-dlp, ffmpeg and deno, if "Get them" downloaded them |
| `backups/` | The library as it was before each import |
| `wikipedia.json` | What Wikipedia said about each song (asked once; "nothing found" again after 30 days) |
| `lyrics-cache/` | Lyrics found online, kept so each song is looked up only once ("nothing found" is asked again after 30 days). Safe to delete |

## Downloading songs

"Download" (next to the name at the top) takes a link to a song or a playlist from
YouTube, SoundCloud, Bandcamp and many other sites. "Check" shows what's there, with
names you can fix first (songs you already have are marked "you have it" and not
ticked); the songs are saved into the open folder as "Artist - Title" with their names
in the tags, three at a time. The sound is kept exactly as the site has it: `.m4a` from
YouTube, `.mp3` where a site has that. Nothing is converted, which is quicker and loses
no quality; only kinds the player can't play (like Opus) are made into mp3. Closing the window
doesn't stop them; "Cancel all" does (half-downloaded pieces are removed).

This uses three free programs: yt-dlp (finds the music, and updates itself once a
day, since sites change), ffmpeg (puts the sound into its file) and deno (reads YouTube's pages).
If they're on the computer they're used; otherwise "Get them" downloads them once
into the player's `bin` folder (about 250 MB).

## Moving to another computer

**Export** (top right) saves everything the player knows about your songs as one
.zip: names, albums, genres, hearts, plays, lyrics choices, covers, saved lyrics,
loudness, Wikipedia answers and settings. Not the music files. **Import** brings such
a .zip in (what was there is kept in the `backups` folder first); on a new computer,
the first-run window can import it right away.

## Changing songs

Point at a song in "All songs" for its buttons:

- **Pencil**: title, artist, album and genres (with suggestions from your library).
  Saving writes them into library.json and into the file's tags, and (if ticked)
  names the file "Artist - Title". "Use the original names" goes back to the file's own.
- **Trash**: moves the song (and a .lrc next to it) to the Trash.
- **Yellow "!"**: the song is here more than once. Pick which copies go to the Trash;
  the best one is suggested (lossless, then the highest bitrate, then the biggest
  file), and the hearts, plays and lyrics choice of deleted copies go to the one kept.

**Tidy file names** (the wand at the top) names every file "Artist - Title"
(track numbers and extras like "(Official Video)" go) and writes names you gave songs
into their files. Only what's ticked changes; "Undo last rename" puts names back.

## Albums and genres from Wikipedia

The disc button at the top (with a dot when there's something new) shows what
Wikipedia knows about your songs' albums and genres: from the song's own article,
then its album's (also the album named in the song's tags), then the artist's. Bands
the English Wikipedia doesn't know are looked up in the Serbian, Croatian, Bosnian
and Serbo-Croatian ones, and their genres get their English names. Songs added while
the player is open are looked up too. Songs are looked up gently in the background,
and answers are kept in `wikipedia.json`. Missing albums and genres are ticked;
different ones are offered unticked; names you set yourself aren't touched. The
pencil's "Album and genres from Wikipedia" fills in one song.

## Lyrics

"Best match" tries, in this order: a `.lrc` file next to the song, the lyrics
folder, lyrics stored inside the song file, then LRCLIB, lrcmux (YouTube Music, Genius,
KuGou) and NetEase Cloud Music (these three are asked at the same time), and, when none
of them has the song, Tekstovi.net (songs from Serbia, Croatia, Bosnia and around;
plain text, which "Make timing" can time). Names are also tried without accents and in
Latin letters ("Noćni voz" as "Nocni voz"). What's found online is remembered, so a
song is looked up once, and the next song's lyrics are fetched while the current one
plays; picking "Best match" in the menu looks everywhere again.
Timed lyrics win, also those of a slightly different version of the song; the
Timing buttons under them (or G and H) line them up, and saving keeps the new
timing. The menu above the lyrics picks one source for a song instead. Lyrics on
this computer (a .lrc next to the song, or saved ones) always win over online ones.

**Lyrics without timing**: "Make timing" spreads the lines over the song as a first
guess. Then, while the song plays, press Space (or Tap) as each highlighted line
starts: every tap also moves the lines after it to the pace you're setting, so a few
taps are often enough. Click any line to set it to now; Undo takes the last tap back.
Save keeps the timing (the clock button under timed lyrics redoes it the same way).

## Building it yourself

    ./build-appimage.sh

needs Rust (https://rustup.rs), curl and python3, and makes
`dist/MusicPlayer-x86_64.AppImage`. The first build downloads the libraries and
takes several minutes; after a change it takes a minute or two. For a quick test
without packing: `cargo run --release`.

## The code

| File | What it does |
| --- | --- |
| `src/main.rs` | Opens the window |
| `src/app.rs` | What the app knows and does: playing, the queue, sorting, filters, favorites, statistics |
| `src/ui.rs` | Everything on screen |
| `src/library.rs` | Finding songs in the folder, reading their tags, noticing changes |
| `src/names.rs` | How songs get their names ("Song (Official Video) [HD]" -> "Song") |
| `src/store.rs` | `library.json`, settings, preferences, importing the old player's data |
| `src/covers.rs` | Covers from the file, its folder, or online (iTunes, Deezer, MusicBrainz) |
| `src/player.rs` | Sound |
| `src/lyrics.rs` | Lyrics: the sources, timing, saving |
| `src/edit.rs` | Changing files: tags, renaming and undo, the Trash |
| `src/wiki.rs` | Albums and genres from Wikipedia (reading infoboxes in wikitext) |
| `src/downloads.rs` | Downloads with yt-dlp, getting the tools |
| `src/transfer.rs` | Export and import: the library as one .zip |
| `src/analysis.rs` | The equalizer (listens to the music on its way to the speakers) |
| `src/loudness.rs` | Even volume (measures songs the way streaming services do) |
| `src/media.rs` | Media keys and the desktop's media controls |
| `src/theme.rs`, `src/genres.rs` | The 13 themes and the genre names (made from the old player's files) |
| `packaging/` | The AppImage's menu entry, and `weak-glibc.py` (lets it run on older systems; see inside) |
| `assets/` | The app icon and the Inter font (open font license in `Inter-LICENSE.txt`) |

Set `MP_DEBUG=1` to see startup steps printed in a terminal.

## Not yet

`.webm` and `.opus` songs don't play yet (the audio library has no Opus decoder).
