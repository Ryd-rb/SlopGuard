#[cfg(feature = "with-beta")]
pub fn number() -> u32 {
    beta::number()
}

#[cfg(not(feature = "with-beta"))]
pub fn number() -> u32 {
    8
}

#[cfg(test)]
mod tests {
    #[test]
    fn passes() {
        assert_eq!(super::number(), 8);
    }
}
