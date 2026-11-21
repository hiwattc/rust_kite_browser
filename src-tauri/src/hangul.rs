//! Korean typed on the wrong input source: `햣` was meant to be `git` (2-beolsik layout).
//! The palette searches both the text and its QWERTY reading.

const CHO: [&str; 19] = ["r", "R", "s", "e", "E", "f", "a", "q", "Q", "t", "T", "d", "w", "W", "c", "z", "x", "v", "g"];
const JUNG: [&str; 21] = ["k", "o", "i", "O", "j", "p", "u", "P", "h", "hk", "ho", "hl", "y", "n", "nj", "np", "nl", "b", "m", "ml", "l"];
const JONG: [&str; 28] = ["", "r", "R", "rt", "s", "sw", "sg", "e", "f", "fr", "fa", "fq", "ft", "fx", "fv", "fg", "a", "q", "qt", "t", "T", "d", "w", "c", "z", "x", "v", "g"];
/// Compatibility jamo U+3131..=U+3163 (ㄱ … ㅣ).
const COMPAT: [&str; 51] = [
    "r", "R", "rt", "s", "sw", "sg", "e", "E", "f", "fr", "fa", "fq", "ft", "fx", "fv", "fg", "a", "q", "Q", "qt", "t", "T", "d", "w", "W", "c", "z", "x", "v", "g",
    "k", "o", "i", "O", "j", "p", "u", "P", "h", "hk", "ho", "hl", "y", "n", "nj", "np", "nl", "b", "m", "ml", "l",
];

/// QWERTY keys for Hangul text, lowercased; `None` when there is no Hangul in it.
pub fn to_qwerty(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut any = false;
    for c in s.chars() {
        let u = c as u32;
        if (0xAC00..=0xD7A3).contains(&u) {
            let i = u - 0xAC00;
            out.push_str(CHO[(i / 588) as usize]);
            out.push_str(JUNG[((i % 588) / 28) as usize]);
            out.push_str(JONG[(i % 28) as usize]);
            any = true;
        } else if (0x3131..=0x3163).contains(&u) {
            out.push_str(COMPAT[(u - 0x3131) as usize]);
            any = true;
        } else {
            out.push(c);
        }
    }
    any.then(|| out.to_lowercase())
}


/// The other way: `qntks` (typed with the English input source on) → `부산`. Only for text made
/// of letters and spaces; `None` when it does not form Hangul.
pub fn from_qwerty(s: &str) -> Option<String> {
    if s.trim().is_empty() || !s.chars().all(|c| c.is_ascii_alphabetic() || c == ' ') {
        return None;
    }
    // Jamo index in COMPAT order (0..=29 consonants, 30..=50 vowels) for each key.
    fn jamo(c: char) -> Option<usize> {
        let key = match c {
            'Q' | 'W' | 'E' | 'R' | 'T' | 'O' | 'P' => c.to_string(),
            _ => c.to_ascii_lowercase().to_string(),
        };
        COMPAT.iter().position(|k| *k == key)
    }
    const CHO_C: [usize; 19] = [0, 1, 3, 6, 7, 8, 16, 17, 18, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29];
    const JONG_C: [usize; 28] = [99, 0, 1, 2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 19, 20, 21, 22, 23, 25, 26, 27, 28, 29];
    let is_vowel = |j: usize| j >= 30;
    let cho_of = |j: usize| CHO_C.iter().position(|x| *x == j);
    let jong_of = |j: usize| JONG_C.iter().position(|x| *x == j);
    let jung_of = |j: usize| j - 30;
    // Two jamo that combine (vowel pairs, final consonant pairs) → combined jamo.
    let combine = |a: usize, b: usize| -> Option<usize> {
        let pair = format!("{}{}", COMPAT[a], COMPAT[b]);
        COMPAT.iter().position(|k| *k == pair)
    };
    let mut out = String::new();
    let mut any = false;
    // Current syllable: (cho, jung, jong) as COMPAT indices.
    let (mut cho, mut jung, mut jong): (Option<usize>, Option<usize>, Option<usize>) = (None, None, None);
    let flush = |out: &mut String, cho: Option<usize>, jung: Option<usize>, jong: Option<usize>, any: &mut bool| match (cho, jung) {
        (Some(c), Some(v)) => {
            let l = cho_of(c).unwrap_or(0) as u32;
            let t = jong.and_then(jong_of).unwrap_or(0) as u32;
            out.push(char::from_u32(0xAC00 + (l * 21 + jung_of(v) as u32) * 28 + t).unwrap_or('?'));
            *any = true;
        }
        (Some(c), None) => out.push(char::from_u32(0x3131 + c as u32).unwrap_or('?')),
        (None, Some(v)) => out.push(char::from_u32(0x3131 + v as u32).unwrap_or('?')),
        _ => {}
    };
    for ch in s.chars() {
        if ch == ' ' {
            flush(&mut out, cho, jung, jong, &mut any);
            (cho, jung, jong) = (None, None, None);
            out.push(' ');
            continue;
        }
        let j = jamo(ch)?;
        if is_vowel(j) {
            match (cho, jung, jong) {
                // ㄱ + ㅏ
                (Some(_), None, None) => jung = Some(j),
                // 고 + ㅏ = 과
                (_, Some(v), None) if combine(v, j).is_some() => jung = combine(v, j),
                // 각 + ㅏ = 가 + 가 (the final moves on; a double final splits)
                (Some(_), Some(_), Some(f)) => {
                    let parts = COMPAT[f];
                    let (keep, moved) = if parts.len() == 2 {
                        (COMPAT.iter().position(|k| *k == &parts[..1]), COMPAT.iter().position(|k| *k == &parts[1..]))
                    } else {
                        (None, Some(f))
                    };
                    flush(&mut out, cho, jung, keep, &mut any);
                    (cho, jung, jong) = (moved, Some(j), None);
                }
                _ => {
                    flush(&mut out, cho, jung, jong, &mut any);
                    (cho, jung, jong) = (None, Some(j), None);
                }
            }
        } else {
            match (cho, jung, jong) {
                (Some(_), Some(_), None) if jong_of(j).is_some() => jong = Some(j),
                (Some(_), Some(_), Some(f)) if combine(f, j).is_some_and(|x| jong_of(x).is_some()) => jong = combine(f, j),
                _ => {
                    flush(&mut out, cho, jung, jong, &mut any);
                    (cho, jung, jong) = (cho_of(j).map(|_| j), None, None);
                    if cho.is_none() {
                        out.push(char::from_u32(0x3131 + j as u32).unwrap_or('?'));
                    }
                }
            }
        }
    }
    flush(&mut out, cho, jung, jong, &mut any);
    any.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrong_input_source() {
        assert_eq!(to_qwerty("햣").as_deref(), Some("git"));
        
        assert_eq!(to_qwerty("쟈소ㅕㅠ").as_deref(), Some("withub"));
        assert_eq!(to_qwerty("ㅜㅁㅍㄷㄱ").as_deref(), Some("naver"));
        assert_eq!(to_qwerty("채ㅕㅅㄷ").as_deref(), Some("coute"));
        assert_eq!(to_qwerty("github"), None);
        assert_eq!(to_qwerty("ㅎㅑ소ㅕㅠ.채ㅡ").as_deref(), Some("github.com"));
    }

    #[test]
    fn english_input_source() {
        assert_eq!(from_qwerty("qntks").as_deref(), Some("부산"));
        assert_eq!(from_qwerty("dkssudgktpdy").as_deref(), Some("안녕하세요"));
        assert_eq!(from_qwerty("rhkdqhr").as_deref(), Some("광복"));
        assert_eq!(from_qwerty("dlfrdj").as_deref(), Some("읽어"));
        assert_eq!(from_qwerty("tjdnf xmrqufdl").as_deref(), Some("서울 특별이"));
        assert_eq!(from_qwerty("Tkd").as_deref(), Some("쌍"));
        assert_eq!(from_qwerty("gks").as_deref(), Some("한"));
        assert_eq!(from_qwerty("github.com"), None);
        assert_eq!(from_qwerty("123"), None);
        // round trip
        for w in ["부산광역시", "깃허브", "개발자 도구", "읽기", "값"] {
            assert_eq!(from_qwerty(&to_qwerty(w).unwrap()).as_deref(), Some(w), "{w}");
        }
    }
}
