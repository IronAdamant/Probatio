pub fn gate(n: i32) -> bool {
    n > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_stays_shut() {
        assert!(!gate(0));
    }
}
