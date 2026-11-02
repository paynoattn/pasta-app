//! The 24-word sync-chain code (#20): BIP39 encoding of 256 bits of entropy.
//!
//! 256 bits plus an 8-bit SHA-256 checksum make 264 bits, 24 words of 11 bits
//! from the BIP39 English list. The words are only ever an encoding of the
//! chain secret; no BIP39 seed (PBKDF2 / passphrase) is derived from them.
//!
//! Typing is forgiving: case, extra whitespace, commas and numbering are
//! ignored, and since every BIP39 word is unique in its first four letters a
//! word can be cut short to those four. The checksum catches a wrong word.

use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::secret::Secret32;

pub const WORD_COUNT: usize = 24;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WordsError {
    #[error("expected {WORD_COUNT} words, got {0}")]
    Count(usize),
    /// 1-based position, so the UI can point at it. Never quotes the word.
    #[error("word {0} isn't in the word list")]
    Unknown(usize),
    #[error("the words don't add up (checksum); check for a typo or a swapped word")]
    Checksum,
}

fn word_list() -> &'static [&'static str; 2048] {
    bip39::Language::English.word_list()
}

/// The 24 words for `secret`, separated by single spaces.
pub fn encode(secret: &Secret32) -> Zeroizing<String> {
    let entropy = secret.expose();
    let checksum = Sha256::digest(entropy)[0];
    let mut bits = Zeroizing::new([0u8; 33]);
    bits[..32].copy_from_slice(entropy);
    bits[32] = checksum;
    let mut out = Zeroizing::new(String::with_capacity(WORD_COUNT * 9));
    for i in 0..WORD_COUNT {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(word_list()[read_11(&bits[..], i)]);
    }
    out
}

/// Parses typed words back into the secret.
pub fn decode(input: &str) -> Result<Secret32, WordsError> {
    let words: Vec<&str> = input
        .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .map(|word| word.trim_matches(|c: char| !c.is_alphabetic()))
        .filter(|word| !word.is_empty())
        .collect();
    if words.len() != WORD_COUNT {
        return Err(WordsError::Count(words.len()));
    }
    let mut indices = Zeroizing::new([0u16; WORD_COUNT]);
    for (i, word) in words.iter().enumerate() {
        indices[i] = find(word).ok_or(WordsError::Unknown(i + 1))?;
    }
    let mut bits = Zeroizing::new([0u8; 33]);
    for (i, index) in indices.iter().enumerate() {
        write_11(&mut bits[..], i, *index);
    }
    let mut entropy = [0u8; 32];
    entropy.copy_from_slice(&bits[..32]);
    let checksum = Sha256::digest(entropy)[0];
    if checksum != bits[32] {
        entropy.zeroize();
        return Err(WordsError::Checksum);
    }
    Ok(Secret32::from_bytes(&mut entropy))
}

/// Index of a word, accepting any unambiguous prefix of at least four letters.
fn find(word: &str) -> Option<u16> {
    let lower = Zeroizing::new(word.to_lowercase());
    let list = word_list();
    if let Ok(index) = list.binary_search(&lower.as_str()) {
        return u16::try_from(index).ok();
    }
    if lower.chars().count() < 4 {
        return None;
    }
    let mut matches = list.iter().enumerate().filter(|(_, candidate)| candidate.starts_with(lower.as_str()));
    match (matches.next(), matches.next()) {
        (Some((index, _)), None) => u16::try_from(index).ok(),
        _ => None,
    }
}

fn read_11(bits: &[u8], index: usize) -> usize {
    let mut value = 0usize;
    for bit in index * 11..index * 11 + 11 {
        value = (value << 1) | usize::from((bits[bit / 8] >> (7 - bit % 8)) & 1);
    }
    value
}

fn write_11(bits: &mut [u8], index: usize, value: u16) {
    for offset in 0..11 {
        let bit = index * 11 + offset;
        if (value >> (10 - offset)) & 1 == 1 {
            bits[bit / 8] |= 1 << (7 - bit % 8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The BIP39 reference vector for 256 bits of 0x7f.
    const SEVENTY_F: &str = "legal winner thank year wave sausage worth useful legal winner thank year wave sausage \
                             worth useful legal winner thank year wave sausage worth title";

    #[test]
    fn matches_the_bip39_reference_vectors() {
        let mut bytes = [0x7f; 32];
        let secret = Secret32::from_bytes(&mut bytes);
        assert_eq!(encode(&secret).as_str(), SEVENTY_F.split_whitespace().collect::<Vec<_>>().join(" "));

        let mut zeros = [0u8; 32];
        let zero = Secret32::from_bytes(&mut zeros);
        let words = encode(&zero);
        assert!(words.starts_with("abandon abandon"), "{}", words.as_str());
        assert!(words.ends_with(" art"), "{}", words.as_str());
    }

    #[test]
    fn round_trips_random_secrets() {
        for _ in 0..50 {
            let secret = Secret32::random();
            assert_eq!(decode(&encode(&secret)).unwrap(), secret);
        }
    }

    #[test]
    fn forgives_formatting_and_short_words() {
        let mut bytes = [0x7f; 32];
        let secret = Secret32::from_bytes(&mut bytes);
        let messy: String = SEVENTY_F
            .split_whitespace()
            .enumerate()
            .map(|(i, word)| {
                let word = if i % 2 == 0 { &word[..word.len().min(4)] } else { word };
                format!("{}. {}", i + 1, word.to_uppercase())
            })
            .collect::<Vec<_>>()
            .join(",\n ");
        assert_eq!(decode(&messy).unwrap(), secret);
    }

    #[test]
    fn reports_mistakes_without_quoting_words() {
        let words: Vec<&str> = SEVENTY_F.split_whitespace().collect();
        assert_eq!(decode(&words[..23].join(" ")), Err(WordsError::Count(23)));

        let mut typo = words.clone();
        typo[4] = "wavx";
        let err = decode(&typo.join(" ")).unwrap_err();
        assert_eq!(err, WordsError::Unknown(5));
        assert!(!err.to_string().contains("wavx"));

        let mut swapped = words.clone();
        swapped.swap(0, 2);
        assert_eq!(decode(&swapped.join(" ")), Err(WordsError::Checksum));
        // Under four letters and not a word itself.
        let mut short = words;
        short[0] = "lex";
        assert_eq!(decode(&short.join(" ")), Err(WordsError::Unknown(1)));
    }
}
