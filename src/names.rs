// How songs get their names: the same rules as the web version, so a library carried
// over from it looks exactly the same.
//
// "Song (Official Video) [HD]"      ->  "Song"
// "Song (Official Live Video)"      ->  "Song (Live)"       (words that matter stay)
// "TaylorSwiftVEVO"                 ->  "Taylor Swift"
// "rock; POP / Indie"               ->  ["Rock", "Pop", "Indie"]
use crate::genres::{GENRE_NAMES, ID3_GENRES};
use regex::Regex;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

pub const UNKNOWN_ARTIST: &str = "Unknown artist";

// A song's four names
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Names {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String, // "Rock, Indie"
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid pattern")
}

// A bracket with any of these goes away completely: "(Lyrics)", "(Prod. by X)", "(www.site.com)"
static DROP_BRACKET: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)lyric|tekst|letra|prevod|prijevod|translat|tradu[cz]|legendad|subtit|on screen|copyright|(?-u:\b)prod(?-u:\b)|www\.|https?:|\.(com|net|org|ru|info)(?-u:\b)")
});
// Inside brackets, these phrases and words are just packaging: "(Official Music Video)", "[HD]"
static EXTRA_PHRASES: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)(?-u:\b)(music video|video clip|high quality|audio only|free download|out now|ncs release)(?-u:\b)")
});
static EXTRA_WORD: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)^(official|oficial|officiel|offizielles?|zvani[cč]ni|video|videoclip|videoklip|clip|spot|audio|visuali[sz]er|hd|hq|uhd|4k|8k|8d|\d{3,4}p|upgrade|mv|m/v|explicit|premiere|new|free|nova|novo|nuevo|pesma|pjesma|full|song|flac|hqflac|mp3|wav|aac|lossless|hires|\d*kbps)$")
});
// Words that don't count as "something left" in a bracket on their own
static FILLER_WORD: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^(the|and|with|version|by|a|of|i|x|&|\+)$"));
// The same packaging at the very end of a name, without brackets: "Song HD", "Song - Official Video"
static TRAILING_EXTRAS: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)(^|[\s|:,/-])(official( music| lyric| hd)? video|official audio|official|lyric video|music video|video oficial|videoclip|lyrics on screen|with lyrics|lyrics|tekst|hd|hq|4k|1080p|720p|youtube)$")
});
static BRACKET: LazyLock<Regex> = LazyLock::new(|| re(r"[(\[{【]([^()\[\]{}【】]*)[)\]}】]"));
static PART_SEPARATOR: LazyLock<Regex> = LazyLock::new(|| re(r"\s(?:\||//)\s"));
static EDGES: LazyLock<Regex> = LazyLock::new(|| re(r"^[\s|:,-]+|[\s|:,\-♪♫•]+$"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| re(r"\s+"));
static LONG_DASH: LazyLock<Regex> = LazyLock::new(|| re(r"\s+[\u2013\u2014]\s+"));
static TRACK_NUMBER: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:\d{1,2}[-.]\d{1,3}\s+|\d{1,3}\s*[.\-)]\s*|0\d\s+)"));
// A YouTube video id that a download left in the name: "[dHC6l7v-1xY]"
static VIDEO_ID: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z0-9_-]{11}$"));
// " - " with a space missing on one side: "Artist- Title", "Artist -Title" (not "Jay-Z")
static LOOSE_DASH: LazyLock<Regex> = LazyLock::new(|| re(r"(\S)- (\S)|(\S) -(\S)"));
static NOT_WORD_CHAR: LazyLock<Regex> = LazyLock::new(|| re(r"[^\p{L}\p{N}/]"));
static HAS_LETTER: LazyLock<Regex> = LazyLock::new(|| re(r"\p{L}"));
static TOPIC: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\s-\stopic$"));
static VEVO: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)vevo$"));
static CAMEL: LazyLock<Regex> = LazyLock::new(|| re(r"(\p{Ll})(\p{Lu})"));
static CHANNEL: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)vevo$|\s-\stopic$"));
static GENRE_SPLIT: LazyLock<Regex> = LazyLock::new(|| re(r"[,;|\x00]+|\s/\s"));
static GENRE_CODE: LazyLock<Regex> = LazyLock::new(|| re(r"^\((\d+)\)(.*)$|^(\d+)$"));
static NOT_A_GENRE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^(other|unknown|genre|music|none|misc)$"));
static WORD_START: LazyLock<Regex> = LazyLock::new(|| re(r"(^|[\s&/+-])(\p{L})"));
static ANY_BRACKET: LazyLock<Regex> = LazyLock::new(|| re(r"[(\[{【][^)\]}】]*[)\]}】]"));
static FEATURING: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\s(feat\.?|ft\.?|featuring)\s.*$"));

// Underscores and long dashes become plain spaces and " - ": "Artist_-_Song" -> "Artist - Song"
pub fn normalize_separators(text: &str) -> String {
    let text = text.replace('_', " ").replace(" ~ ", " - ").replace(" -- ", " - ");
    let text = LONG_DASH.replace_all(&text, " - ");
    let text = LOOSE_DASH.replace_all(&text, |caps: &regex::Captures| match (caps.get(1), caps.get(3)) {
        (Some(a), _) => format!("{} - {}", a.as_str(), &caps[2]),
        (None, Some(a)) => format!("{} - {}", a.as_str(), &caps[4]),
        _ => caps[0].to_string(),
    });
    SPACES.replace_all(&text, " ").trim().to_string()
}

// Called for each bracket: keeps it, trims it, or drops it
fn tidy_bracket(whole: &str, inside: &str) -> String {
    if DROP_BRACKET.is_match(inside) || is_video_id(inside.trim()) {
        return " ".into();
    }
    let without_phrases = EXTRA_PHRASES.replace_all(inside, " ");
    let words: Vec<&str> = without_phrases
        .split(|c: char| c.is_whitespace() || c == '-' && inside.chars().filter(|c| c.is_alphabetic()).all(char::is_uppercase))
        .filter(|word| !word.is_empty())
        .filter(|word| !EXTRA_WORD.is_match(&NOT_WORD_CHAR.replace_all(word, "")))
        .collect();
    let something_left = words.iter().any(|word| HAS_LETTER.is_match(word) && !FILLER_WORD.is_match(word));
    if !something_left {
        return " ".into();
    }
    let open = whole.chars().next().unwrap_or('(');
    let close = whole.chars().last().unwrap_or(')');
    format!("{open}{}{close}", words.join(" "))
}

pub fn clean_name(text: &str) -> String {
    let original = normalize_separators(text);
    // "Song | Label" or "Song // Lyrics": keep the first part
    let mut name = PART_SEPARATOR.split(&original).next().unwrap_or("").to_string();
    loop {
        // brackets inside brackets need more than one pass
        let next = BRACKET
            .replace_all(&name, |caps: &regex::Captures| tidy_bracket(&caps[0], &caps[1]))
            .to_string();
        if next == name {
            break;
        }
        name = next;
    }
    loop {
        let next = TRAILING_EXTRAS.replace(&name, "").trim().to_string();
        if next == name {
            break;
        }
        name = next;
    }
    let name = SPACES.replace_all(&name, " ");
    let name = EDGES.replace_all(&name, "").trim().to_string();
    if name.is_empty() { original } else { name } // never turn a name into nothing
}

// "dHC6l7v-1xY" is a video id; "Instrumental" or "Unplugged" aren't (ids mix in digits,
// dashes, or upper and lower case in the middle)
fn is_video_id(text: &str) -> bool {
    if !VIDEO_ID.is_match(text) {
        return false;
    }
    let digits_or_dashes = text.chars().any(|c| c.is_ascii_digit() || c == '-' || c == '_');
    let inner_capitals = text.chars().skip(1).filter(char::is_ascii_uppercase).count();
    let lowercase = text.chars().filter(char::is_ascii_lowercase).count();
    digits_or_dashes || (inner_capitals >= 2 && lowercase >= 2)
}

// Artists: also fixes YouTube channel names like "Artist - Topic" and "TaylorSwiftVEVO"
pub fn clean_artist(name: &str) -> String {
    let mut artist = TOPIC.replace(&clean_name(name), "").to_string();
    if VEVO.is_match(&artist) {
        let without = VEVO.replace(&artist, "").to_string();
        artist = CAMEL.replace_all(&without, "$1 $2").to_string();
    }
    artist.trim().to_string()
}

// "rock; POP / Indie" -> ["Rock", "Pop", "Indie"] (at most six, without "Other" and the like)
pub fn clean_genres(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for part in GENRE_SPLIT.split(text) {
        let mut name = part.trim().to_string();
        if let Some(caps) = GENRE_CODE.captures(&name) {
            let (number, rest) = match (caps.get(1), caps.get(3)) {
                (Some(number), _) => (number.as_str(), caps.get(2).map_or("", |m| m.as_str()).trim()),
                (None, Some(number)) => (number.as_str(), ""),
                _ => ("", ""),
            };
            name = if !rest.is_empty() {
                rest.to_string()
            } else {
                number.parse::<usize>().ok().and_then(|i| ID3_GENRES.get(i)).map_or(String::new(), |g| g.to_string())
            };
        }
        let known = genre_alias(&name.to_lowercase());
        let pieces: Vec<String> = match known {
            Some(known) => vec![known.to_string()],
            None => name.split('/').map(str::to_string).collect(), // "Rock/Pop" is two genres
        };
        for piece in pieces {
            let piece = SPACES.replace_all(piece.trim(), " ").to_string();
            if piece.is_empty() || NOT_A_GENRE.is_match(&piece) {
                continue;
            }
            let piece = match genre_alias(&piece.to_lowercase()) {
                Some(alias) => alias.to_string(),
                None if piece == piece.to_lowercase() || piece == piece.to_uppercase() => WORD_START
                    .replace_all(&piece.to_lowercase(), |caps: &regex::Captures| {
                        format!("{}{}", &caps[1], caps[2].to_uppercase())
                    })
                    .to_string(),
                None => piece,
            };
            if !names.iter().any(|other| other.to_lowercase() == piece.to_lowercase()) {
                names.push(piece);
            }
        }
    }
    names.truncate(6);
    names
}

fn genre_alias(lower: &str) -> Option<&'static str> {
    GENRE_NAMES.iter().find(|(alias, _)| *alias == lower).map(|(_, name)| *name)
}

// "Music/Album/01 - Artist - Title.mp3" -> artist "Artist", title "Title", album "Album"
// (path: the song's place inside the music folder)
pub fn guess_from_file_name(path: &str) -> Names {
    let parts: Vec<&str> = path.split('/').collect();
    let file = parts.last().copied().unwrap_or("");
    let stem = match file.rfind('.') {
        Some(dot) if dot > 0 => &file[..dot],
        _ => file,
    };
    let name = TRACK_NUMBER.replace(&normalize_separators(stem), "").to_string();
    let name = BRACKET.replace_all(&name, |caps: &regex::Captures| tidy_bracket(&caps[0], &caps[1])).trim().to_string();
    let mut album = if parts.len() > 1 { parts[parts.len() - 2].to_string() } else { String::new() };
    // A library sorted into folders knows the artist when the file name doesn't:
    // "Artist - Album/01 Song.mp3", or "Artist/Album/01 Song.mp3"
    let mut folder_artist = String::new();
    if let Some(dash) = album.find(" - ") {
        folder_artist = album[..dash].trim().to_string();
        album = album[dash + 3..].trim().to_string();
    } else if parts.len() > 2 {
        folder_artist = parts[parts.len() - 3].to_string();
    }
    match name.find(" - ") {
        None if !folder_artist.is_empty() => Names { title: name, artist: folder_artist, album, genre: String::new() },
        None => Names { title: name, artist: UNKNOWN_ARTIST.into(), album, genre: String::new() },
        Some(dash) => Names {
            artist: name[..dash].trim().to_string(),
            title: name[dash + 3..].trim().to_string(),
            album,
            genre: String::new(),
        },
    }
}

// A song's own names: from its tags, or guessed from the file name when it has none.
// Downloaded videos often have "Artist - Title" in the title and a channel name as the artist.
pub fn natural_names(tags: &Names, path: &str) -> Names {
    let guess = guess_from_file_name(path);
    let pick = |tag: &str, guessed: &str| if tag.is_empty() { guessed.to_string() } else { tag.to_string() };
    let mut artist = pick(&tags.artist, &guess.artist);
    let mut title = normalize_separators(&pick(&tags.title, &guess.title));
    let album = pick(&tags.album, &guess.album);
    if let Some(dash) = title.find(" - ").filter(|&dash| dash > 0) {
        let first = title[..dash].to_string();
        if artist.is_empty() || artist == UNKNOWN_ARTIST || same_name(&first, &artist) || CHANNEL.is_match(&artist) {
            artist = first;
            title = title[dash + 3..].to_string();
        }
    }
    let artist = clean_artist(&artist);
    Names {
        title: clean_name(&title),
        artist: if artist.is_empty() { UNKNOWN_ARTIST.into() } else { artist },
        album: if album.is_empty() { String::new() } else { clean_name(&album) },
        genre: clean_genres(&tags.genre).join(", "),
    }
}

// Lowercase and without accents, so "sta" finds "Šta" and "dorde" finds "Đorđe"
pub fn normalize(text: &str) -> String {
    text.to_lowercase()
        .nfd()
        .filter(|c| !('\u{300}'..='\u{36f}').contains(c))
        .map(|c| if c == 'đ' { 'd' } else { c })
        .collect()
}

// Letters and digits only, without accents ("Noćni Voz!" -> "nocnivoz"), the way the
// old server.py named cached covers, so they're found again
pub fn simplify(text: &str) -> String {
    text.to_lowercase().replace('đ', "d").nfkd().filter(|c| c.is_alphanumeric()).collect()
}

// True when one name contains the other, ignoring case, accents and punctuation
pub fn same_name(a: &str, b: &str) -> bool {
    let simple = |text: &str| normalize(text).chars().filter(|c| c.is_alphanumeric()).collect::<String>();
    let (x, y) = (simple(a), simple(b));
    !x.is_empty() && !y.is_empty() && (x.contains(&y) || y.contains(&x))
}

// For looking up covers: "Song (Live) feat. Someone" -> "Song"
pub fn lookup_name(text: &str) -> String {
    let simple = ANY_BRACKET.replace_all(text, " ");
    let simple = FEATURING.replace(&simple, "");
    let simple = SPACES.replace_all(&simple, " ").trim().to_string();
    if simple.is_empty() { text.to_string() } else { simple }
}

pub fn genres_of(genre: &str) -> Vec<&str> {
    if genre.is_empty() { Vec::new() } else { genre.split(", ").collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_like_the_web_version() {
        assert_eq!(clean_name("Song (Official Video) [HD]"), "Song");
        assert_eq!(clean_name("Song (Official Live Video)"), "Song (Live)");
        assert_eq!(clean_name("Numb (Lyrics) | Linkin Park"), "Numb");
        assert_eq!(clean_name("Song - Official Video"), "Song");
        assert_eq!(clean_artist("TaylorSwiftVEVO"), "Taylor Swift");
        assert_eq!(clean_artist("Metallica - Topic"), "Metallica");
        assert_eq!(clean_genres("rock; POP / Indie"), vec!["Rock", "Pop", "Indie"]);
        assert_eq!(clean_genres("(17)"), vec!["Rock"]);
        assert_eq!(clean_genres("hip hop"), vec!["Hip-Hop"]);
        assert_eq!(normalize("Đorđe Šta"), "dorde sta");
        assert_eq!(simplify("Noćni Voz!"), "nocnivoz");
        let guess = guess_from_file_name("Album/01 - Artist - Title.mp3");
        assert_eq!((guess.artist.as_str(), guess.title.as_str(), guess.album.as_str()), ("Artist", "Title", "Album"));
        let tags = Names { title: "Carry On (Official Video)".into(), artist: "AvengedSevenfoldVEVO".into(), ..Default::default() };
        let names = natural_names(&tags, "x.mp3");
        assert_eq!((names.artist.as_str(), names.title.as_str()), ("Avenged Sevenfold", "Carry On"));
        assert_eq!(lookup_name("Song (Live) feat. Someone"), "Song");
        // Better at finding names in messy file names
        let names = |path: &str| { let n = natural_names(&Names::default(), path); (n.artist, n.title, n.album) };
        assert_eq!(names("[HQ-FLAC] Metallica - The Unforgiven II.mp3"), ("Metallica".into(), "The Unforgiven II".into(), "".into()));
        assert_eq!(names("Arcade Fire - Put Your Money on Me [dHC6l7v-1xY].mp3").1, "Put Your Money on Me");
        assert_eq!(names("Carry On [El-qHgSWt3E].mp3").1, "Carry On");
        assert_eq!(names("Muse- Starlight (320 kbps).mp3"), ("Muse".into(), "Starlight".into(), "".into()));
        assert_eq!(names("Metallica/Black Album/01 Enter Sandman.flac"), ("Metallica".into(), "Enter Sandman".into(), "Black Album".into()));
        assert_eq!(names("Muse - Origin of Symmetry/1-03 Plug In Baby.mp3"), ("Muse".into(), "Plug In Baby".into(), "Origin of Symmetry".into()));
        assert_eq!(names("99 Problems.mp3").1, "99 Problems");
        assert_eq!(names("Jay-Z - Encore.mp3").0, "Jay-Z");
        assert_eq!(names("Song (Instrumental).mp3").1, "Song (Instrumental)");
    }
}
