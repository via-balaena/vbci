//! VBCI's records, typed. Each schema refuses a key it doesn't know, and each loader checks the
//! rules its record's comments state. Parsing a record needs no database and no network.

pub mod diet;
pub mod nutrients;
pub mod sources;

pub use diet::{Answer, Class, Diet, DietCategory, Exception, Organism, Rule, Taxon};
pub use nutrients::{Nutrient, Nutrients};
pub use sources::{Dataset, Publisher, Sources};
