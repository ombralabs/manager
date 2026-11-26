pub mod generate;
pub mod locale;
pub mod model;
pub mod resolve;
pub mod validate;

pub use generate::{Catalog, Combo, GenerateError, OsRandom, RandomSource, generate, regenerate};
pub use model::{Field, Fingerprint, Gpu, Location, Os, Screen, Setting, WebRtc};
pub use resolve::{GeoFacts, ResolveError, Resolved, needs_geo, resolve};
pub use validate::{Arch, FieldError, is_apple_gpu, validate};
