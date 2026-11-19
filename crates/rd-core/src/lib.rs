pub mod identity;
pub mod media;
pub mod protocol;

pub fn version() -> &'static str {
    "rd-core v0.1.0"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_not_empty() {
        assert!(!version().is_empty());
    }
}
