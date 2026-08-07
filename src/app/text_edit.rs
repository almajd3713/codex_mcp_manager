use unicode_width::UnicodeWidthChar;

pub(super) fn end(value: &str) -> usize {
    value.chars().count()
}

pub(super) fn move_left(cursor: &mut usize) {
    *cursor = cursor.saturating_sub(1);
}

pub(super) fn move_right(value: &str, cursor: &mut usize) {
    *cursor = (*cursor + 1).min(end(value));
}

pub(super) fn insert(value: &mut String, cursor: &mut usize, character: char) {
    let byte = byte_index(value, *cursor);
    value.insert(byte, character);
    *cursor += 1;
}

pub(super) fn backspace(value: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    let start = byte_index(value, *cursor - 1);
    let end = byte_index(value, *cursor);
    value.replace_range(start..end, "");
    *cursor -= 1;
}

pub(super) fn delete(value: &mut String, cursor: usize) {
    if cursor >= end(value) {
        return;
    }
    let start = byte_index(value, cursor);
    let end = byte_index(value, cursor + 1);
    value.replace_range(start..end, "");
}

pub(super) fn clear(value: &mut String, cursor: &mut usize) {
    value.clear();
    *cursor = 0;
}

pub(super) fn viewport(value: &str, cursor: usize, width: u16) -> (String, u16) {
    let max_width = usize::from(width.max(1));
    let chars = value.chars().collect::<Vec<_>>();
    let cursor = cursor.min(chars.len());
    let widths = chars
        .iter()
        .map(|character| character.width().unwrap_or(0))
        .collect::<Vec<_>>();
    let cursor_column = widths[..cursor].iter().sum::<usize>();
    let target_start = cursor_column.saturating_sub(max_width.saturating_sub(1));

    let mut start = 0;
    let mut skipped_width = 0;
    while start < cursor && skipped_width + widths[start] <= target_start {
        skipped_width += widths[start];
        start += 1;
    }

    let mut visible = String::new();
    let mut used = 0;
    for (&character, &character_width) in chars[start..].iter().zip(&widths[start..]) {
        if used + character_width > max_width {
            break;
        }
        visible.push(character);
        used += character_width;
    }
    let cursor_x = widths[start..cursor].iter().sum::<usize>().min(max_width);
    (visible, cursor_x as u16)
}

fn byte_index(value: &str, character_index: usize) -> usize {
    value
        .char_indices()
        .nth(character_index)
        .map_or(value.len(), |(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_at_cursor_without_breaking_unicode() {
        let mut value = "ab界d".to_owned();
        let mut cursor = 3;
        backspace(&mut value, &mut cursor);
        insert(&mut value, &mut cursor, 'c');
        delete(&mut value, cursor);
        assert_eq!(value, "abc");
        assert_eq!(cursor, 3);
    }

    #[test]
    fn viewport_keeps_cursor_visible_for_long_text() {
        let (visible, cursor_x) = viewport("0123456789", 8, 5);
        assert_eq!(visible, "45678");
        assert_eq!(cursor_x, 4);
    }
}
