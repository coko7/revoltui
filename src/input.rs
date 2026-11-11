use std::fmt;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Single-line text input with a cursor and readline-style editing keys.
#[derive(Debug, Default, Clone)]
pub struct Input {
    text: String,
    /// Byte offset into `text`, always on a char boundary.
    cursor: usize,
}

impl Input {
    pub fn new(text: String) -> Self {
        let cursor = text.len();
        Self { text, cursor }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Text before the cursor, the char under it (if any), and the text after it.
    pub fn split_at_cursor(&self) -> (&str, Option<char>, &str) {
        let (before, rest) = self.text.split_at(self.cursor);
        let mut chars = rest.chars();
        let at = chars.next();
        (before, at, chars.as_str())
    }

    /// Applies an editing key. Returns `false` when `key` is not one.
    pub fn handle(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            // Most terminals send Ctrl+Backspace as Ctrl+H.
            KeyCode::Char('w' | 'h') if ctrl => self.cut(self.word_start(), self.cursor),
            KeyCode::Backspace if ctrl || alt => self.cut(self.word_start(), self.cursor),
            KeyCode::Backspace => self.cut(self.prev(), self.cursor),
            KeyCode::Delete if ctrl || alt => self.cut(self.cursor, self.word_end()),
            KeyCode::Char('d') if alt => self.cut(self.cursor, self.word_end()),
            KeyCode::Delete => self.cut(self.cursor, self.next()),
            KeyCode::Char('k') if ctrl => self.cut(self.cursor, self.text.len()),
            KeyCode::Left if ctrl || alt => self.cursor = self.word_start(),
            KeyCode::Char('b') if alt => self.cursor = self.word_start(),
            KeyCode::Right if ctrl || alt => self.cursor = self.word_end(),
            KeyCode::Char('f') if alt => self.cursor = self.word_end(),
            KeyCode::Left => self.cursor = self.prev(),
            KeyCode::Right => self.cursor = self.next(),
            KeyCode::Home => self.cursor = 0,
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            KeyCode::Char('e') if ctrl => self.cursor = self.text.len(),
            KeyCode::Char(c) if !ctrl && !alt => {
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            _ => return false,
        }
        true
    }

    fn cut(&mut self, from: usize, to: usize) {
        self.text.replace_range(from..to, "");
        self.cursor = from;
    }

    fn prev(&self) -> usize {
        self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next(&self) -> usize {
        self.text[self.cursor..]
            .chars()
            .next()
            .map_or(self.cursor, |c| self.cursor + c.len_utf8())
    }

    /// Start of the word before the cursor, skipping separators first.
    fn word_start(&self) -> usize {
        let mut start = self.cursor;
        let mut in_word = false;
        for (i, c) in self.text[..self.cursor].char_indices().rev() {
            if c.is_alphanumeric() {
                in_word = true;
            } else if in_word {
                break;
            }
            start = i;
        }
        start
    }

    /// End of the word after the cursor, skipping separators first.
    fn word_end(&self) -> usize {
        let mut in_word = false;
        for (i, c) in self.text[self.cursor..].char_indices() {
            if c.is_alphanumeric() {
                in_word = true;
            } else if in_word {
                return self.cursor + i;
            }
        }
        self.text.len()
    }
}

impl fmt::Display for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn input(text: &str) -> Input {
        Input::new(text.to_string())
    }

    /// Text with `|` at the cursor.
    fn show(i: &Input) -> String {
        let (before, at, after) = i.split_at_cursor();
        format!(
            "{before}|{}{after}",
            at.map(String::from).unwrap_or_default()
        )
    }

    const CTRL: KeyModifiers = KeyModifiers::CONTROL;
    const ALT: KeyModifiers = KeyModifiers::ALT;
    const NONE: KeyModifiers = KeyModifiers::NONE;

    #[test]
    fn deletes_words_backwards() {
        for k in [
            key(KeyCode::Char('w'), CTRL),
            key(KeyCode::Char('h'), CTRL),
            key(KeyCode::Backspace, CTRL),
            key(KeyCode::Backspace, ALT),
        ] {
            let mut i = input("saint-nazaire  ");
            i.handle(k);
            assert_eq!(show(&i), "saint-|", "{k:?}");
            i.handle(k);
            assert_eq!(show(&i), "|", "{k:?}");
        }
    }

    #[test]
    fn moves_by_word_and_char() {
        let mut i = input("la baule escoublac");
        i.handle(key(KeyCode::Left, CTRL));
        assert_eq!(show(&i), "la baule |escoublac");
        i.handle(key(KeyCode::Left, ALT));
        assert_eq!(show(&i), "la |baule escoublac");
        i.handle(key(KeyCode::Char('b'), ALT));
        assert_eq!(show(&i), "|la baule escoublac");
        i.handle(key(KeyCode::Right, CTRL));
        assert_eq!(show(&i), "la| baule escoublac");
        i.handle(key(KeyCode::Right, NONE));
        assert_eq!(show(&i), "la |baule escoublac");
        i.handle(key(KeyCode::End, NONE));
        assert_eq!(show(&i), "la baule escoublac|");
        i.handle(key(KeyCode::Char('a'), CTRL));
        assert_eq!(show(&i), "|la baule escoublac");
    }

    #[test]
    fn edits_at_the_cursor() {
        let mut i = input("bziers");
        i.handle(key(KeyCode::Home, NONE));
        i.handle(key(KeyCode::Right, NONE));
        i.handle(key(KeyCode::Char('é'), NONE));
        assert_eq!(show(&i), "bé|ziers");
        i.handle(key(KeyCode::Left, NONE));
        i.handle(key(KeyCode::Delete, NONE));
        assert_eq!(show(&i), "b|ziers");
        i.handle(key(KeyCode::Backspace, NONE));
        assert_eq!(show(&i), "|ziers");
        i.handle(key(KeyCode::Delete, CTRL));
        assert_eq!(show(&i), "|");

        let mut i = input("saint étienne");
        i.handle(key(KeyCode::Left, CTRL));
        i.handle(key(KeyCode::Char('k'), CTRL));
        assert_eq!(show(&i), "saint |");
        assert!(
            !i.handle(key(KeyCode::Char('u'), CTRL)),
            "not an editing key"
        );
    }
}
