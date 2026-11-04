use rand::seq::IndexedRandom;

const ADJECTIVES: &[&str] = &[
    "able", "amber", "ancient", "bold", "brave", "breezy", "bright", "brisk", "calm", "careful",
    "cheerful", "clever", "cosmic", "cozy", "crisp", "curious", "dapper", "daring", "dusty",
    "eager", "early", "easy", "elegant", "faint", "fancy", "fast", "fierce", "fluffy", "fresh",
    "friendly", "gentle", "giant", "glad", "golden", "grand", "happy", "hardy", "hidden", "honest",
    "humble", "icy", "jolly", "keen", "kind", "lazy", "light", "little", "lively", "lone", "loud",
    "lucky", "mellow", "merry", "mighty", "misty", "modest", "neat", "nimble", "noble", "odd",
    "old", "patient", "peaceful", "plain", "playful", "polite", "proud", "quick", "quiet", "rapid",
    "rare", "rosy", "round", "royal", "rustic", "sandy", "shiny", "shy", "silent", "silver",
    "simple", "sleepy", "slow", "small", "smooth", "snowy", "soft", "solid", "sparkly", "steady",
    "sunny", "swift", "tame", "tidy", "tiny", "warm", "wild", "wise", "witty", "young", "zesty",
];

const NOUNS: &[&str] = &[
    "acorn", "anchor", "apple", "badger", "basket", "beacon", "biscuit", "blanket", "breeze",
    "bridge", "brook", "bucket", "butter", "button", "cabin", "candle", "canyon", "carrot",
    "castle", "cedar", "cherry", "clover", "cloud", "cobble", "comet", "cookie", "coral",
    "cricket", "daisy", "desert", "dolphin", "dragon", "eagle", "ember", "falcon", "feather",
    "fern", "finch", "forest", "fox", "garden", "ginger", "glacier", "harbor", "hazel", "heron",
    "hill", "honey", "island", "jacket", "jelly", "kettle", "kitten", "ladder", "lantern", "lemon",
    "lily", "maple", "meadow", "mitten", "moon", "mountain", "muffin", "noodle", "oasis", "olive",
    "orchard", "otter", "owl", "panda", "pebble", "pepper", "pillow", "pine", "planet", "pond",
    "pumpkin", "puppy", "quill", "rabbit", "raven", "river", "robin", "rocket", "saddle", "sailor",
    "sparrow", "spoon", "squirrel", "star", "stream", "teapot", "thistle", "tiger", "tulip",
    "turtle", "valley", "violin", "walnut", "willow", "window", "wolf", "zebra",
];

pub fn random_name() -> String {
    let mut rng = rand::rng();
    let adjective = ADJECTIVES.choose(&mut rng).unwrap();
    let first = NOUNS.choose(&mut rng).unwrap();
    let second = NOUNS.choose(&mut rng).unwrap();
    format!("{adjective}-{first}-{second}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_have_three_lowercase_parts() {
        for _ in 0..100 {
            let name = random_name();
            let parts: Vec<_> = name.split('-').collect();
            assert_eq!(parts.len(), 3);
            assert!(
                parts
                    .iter()
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase()))
            );
        }
    }
}
