//! VBCI's records, typed. Each schema refuses a key it doesn't know, and each loader checks the
//! rules its record's comments state. Parsing a record needs no database and no network.

pub mod diet;
pub mod ingredients;
pub mod nutrients;
pub mod sources;
pub mod units;

pub use diet::{Answer, Class, Diet, DietCategory, Exception, Organism, Rule, Taxon};
pub use ingredients::{Form, Ingredient, Ingredients, State};
pub use nutrients::{Nutrient, Nutrients};
pub use sources::{Dataset, Publisher, Sources};
pub use units::{CountUnit, CountUnits};
