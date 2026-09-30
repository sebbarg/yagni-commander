/// Returns the first visible row so that `cursor` stays inside a window of
/// `rows` rows, scrolling as little as possible from `offset`.
/// For frontends that render only the visible slice of a listing.
pub fn scroll_offset(offset: usize, cursor: usize, rows: usize, len: usize) -> usize {
    let rows = rows.max(1);
    let max_offset = len.saturating_sub(rows);
    let offset = if cursor < offset {
        cursor
    } else if cursor >= offset + rows {
        cursor + 1 - rows
    } else {
        offset
    };
    offset.min(max_offset)
}

#[cfg(test)]
mod tests {
    use super::scroll_offset;

    #[test]
    fn keeps_offset_while_cursor_visible() {
        assert_eq!(scroll_offset(5, 7, 10, 100), 5);
    }

    #[test]
    fn scrolls_minimally_to_reveal_cursor() {
        assert_eq!(scroll_offset(5, 3, 10, 100), 3);
        assert_eq!(scroll_offset(5, 15, 10, 100), 6);
    }

    #[test]
    fn clamps_when_window_grows_past_end() {
        assert_eq!(scroll_offset(90, 95, 20, 100), 80);
        assert_eq!(scroll_offset(3, 4, 20, 10), 0);
    }
}
