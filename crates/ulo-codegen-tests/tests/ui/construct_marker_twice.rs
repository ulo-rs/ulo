// A field marked `#[construct(default)]` twice, once bare and once under `ulo`. Decision 69.
// Refused at: the second marker, "`#[construct(default)]` is written once".
use ulo::injectable;

#[injectable]
struct Orders {
    #[construct(default)]
    #[ulo::construct(default)]
    limit: u32,
}

fn main() {}
