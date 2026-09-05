pub fn count_lines(text: &str) -> usize {
    text.lines().count()
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty_and_crlf() {
        assert_eq!(super::count_lines(""), 0);
        assert_eq!(super::count_lines("one\r\ntwo\r\n"), 2);
    }
}
