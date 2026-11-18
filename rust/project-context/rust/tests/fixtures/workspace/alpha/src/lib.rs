#[cfg(all(feature = "with-beta", target_os = "linux"))]
pub fn number() -> u32 {
    beta::number()
}

#[cfg(not(all(feature = "with-beta", target_os = "linux")))]
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
