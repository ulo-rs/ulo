use ulo::injectable;

/// Builds the greeting every handler of the project answers with. Injected as `Dep<Greeter>`.
#[injectable]
pub struct Greeter;

impl Greeter {
    pub fn greet(&self, name: &str) -> String {
        format!("Hello, {name}!")
    }
}

#[cfg(test)]
mod tests {
    use super::Greeter;

    #[test]
    fn greets_by_name() {
        assert_eq!(Greeter.greet("Ada"), "Hello, Ada!");
    }
}
