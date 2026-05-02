use chrono::Utc;

pub fn new_id(prefix: &str) -> String {
    let ts = Utc::now().timestamp_millis();
    let rand: u64 = rand::random();
    format!("{}_{}_{}", prefix, ts, rand)
}

/// Server/location names with their icons.
///
/// Servers now use location nouns while sessions use client/entity nouns,
/// producing names like "harbor fox" or "observatory otter".
const SERVER_MODIFIERS: &[(&str, &str)] = &[
    // Natural places
    ("cove", "🌊"),
    ("grove", "🌳"),
    ("meadow", "🌾"),
    ("marsh", "🌿"),
    ("lake", "🏞️"),
    ("river", "🏞️"),
    ("creek", "💧"),
    ("brook", "💧"),
    ("cliff", "🏔️"),
    ("peak", "⛰️"),
    ("summit", "🏔️"),
    ("forest", "🌲"),
    ("garden", "🌷"),
    ("island", "🏝️"),
    ("desert", "🏜️"),
    ("beach", "🏖️"),
    // Built places
    ("harbor", "⚓"),
    ("camp", "⛺"),
    ("forge", "🔥"),
    ("citadel", "🏛️"),
    ("station", "🚉"),
    ("observatory", "🔭"),
    ("workshop", "🛠️"),
    ("lighthouse", "🗼"),
    ("temple", "🏛️"),
    ("castle", "🏰"),
    ("bridge", "🌉"),
    ("fountain", "⛲"),
    ("stadium", "🏟️"),
    ("factory", "🏭"),
    ("pagoda", "🛕"),
    ("hut", "🛖"),
];

/// Session/client names with their icons.
const SESSION_NAMES: &[(&str, &str)] = &[
    // Animals and client entities
    ("ant", "🐜"),
    ("bat", "🦇"),
    ("bee", "🐝"),
    ("bird", "🐦"),
    ("bug", "🐛"),
    ("cat", "🐱"),
    ("chicken", "🐔"),
    ("chick", "🐥"),
    ("chipmunk", "🐿️"),
    ("cockroach", "🪳"),
    ("cow", "🐄"),
    ("crocodile", "🐊"),
    ("cricket", "🦗"),
    ("dodo", "🦤"),
    ("dog", "🐕"),
    ("dove", "🕊️"),
    ("eagle", "🦅"),
    ("falcon", "🦅"),
    ("fish", "🐟"),
    ("fly", "🪰"),
    ("fox", "🦊"),
    ("giraffe", "🦒"),
    ("hamster", "🐹"),
    ("hawk", "🦅"),
    ("ladybug", "🐞"),
    ("lobster", "🦞"),
    ("mammoth", "🦣"),
    ("mosquito", "🦟"),
    ("owl", "🦉"),
    ("ox", "🐂"),
    ("pig", "🐷"),
    ("polar-bear", "🐻‍❄️"),
    ("rat", "🐀"),
    ("ram", "🐏"),
    ("raven", "🐦‍⬛"),
    ("rooster", "🐓"),
    ("shrimp", "🦐"),
    ("sauropod", "🦕"),
    ("blowfish", "🐡"),
    ("buffalo", "🐃"),
    ("butterfly", "🦋"),
    ("badger", "🦡"),
    ("bear", "🐻"),
    ("crab", "🦀"),
    ("crow", "🐦‍⬛"),
    ("deer", "🦌"),
    ("duck", "🦆"),
    ("frog", "🐸"),
    ("goat", "🐐"),
    ("lion", "🦁"),
    ("moth", "🦋"),
    ("wolf", "🐺"),
    ("goose", "🪿"),
    ("horse", "🐴"),
    ("koala", "🐨"),
    ("llama", "🦙"),
    ("moose", "🫎"),
    ("mouse", "🐭"),
    ("otter", "🦦"),
    ("panda", "🐼"),
    ("peacock", "🦚"),
    ("penguin", "🐧"),
    ("shark", "🦈"),
    ("sheep", "🐑"),
    ("sloth", "🦥"),
    ("snail", "🐌"),
    ("snake", "🐍"),
    ("spider", "🕷️"),
    ("squid", "🦑"),
    ("swan", "🦢"),
    ("t-rex", "🦖"),
    ("tiger", "🐯"),
    ("turkey", "🦃"),
    ("whale", "🐋"),
    ("worm", "🪱"),
    ("turtle", "🐢"),
    ("rabbit", "🐰"),
    ("parrot", "🦜"),
    ("jaguar", "🐆"),
    ("lizard", "🦎"),
    ("monkey", "🐒"),
    ("gorilla", "🦍"),
    ("orangutan", "🦧"),
    ("donkey", "🫏"),
    ("camel", "🐫"),
    ("elephant", "🐘"),
    ("rhino", "🦏"),
    ("hippo", "🦛"),
    ("bison", "🦬"),
    ("boar", "🐗"),
    ("unicorn", "🦄"),
    ("kangaroo", "🦘"),
    ("hedgehog", "🦔"),
    ("beaver", "🦫"),
    ("skunk", "🦨"),
    ("raccoon", "🦝"),
    ("seal", "🦭"),
    ("flamingo", "🦩"),
    ("dolphin", "🐬"),
    ("octopus", "🐙"),
    ("jellyfish", "🪼"),
    ("scorpion", "🦂"),
    ("beetle", "🪲"),
    ("zebra", "🦓"),
];

/// Get an emoji icon for a session/client name word.
pub fn session_icon(name: &str) -> &'static str {
    SESSION_NAMES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, icon)| *icon)
        .unwrap_or("💫")
}

/// Get an emoji icon for a server/location name word.
pub fn server_icon(name: &str) -> &'static str {
    SERVER_MODIFIERS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, icon)| *icon)
        .unwrap_or("🔮")
}

/// Generate a memorable server name using a location noun.
/// Returns (full_id, short_name) where:
/// - full_id is the storage identifier like "server_blazing_1234567890_deadbeefcafebabe"
/// - short_name is the memorable part like "blazing"
pub fn new_memorable_server_id() -> (String, String) {
    let ts = Utc::now().timestamp_millis();
    let rand: u64 = rand::random();

    // Use the random value to pick a location noun.
    let idx = (rand as usize) % SERVER_MODIFIERS.len();
    let (word, _) = SERVER_MODIFIERS[idx];

    let short_name = word.to_string();
    let full_id = format!("server_{}_{ts}_{rand:016x}", word);

    (full_id, short_name)
}

/// Try to extract the memorable name from a server ID
/// e.g., "server_blazing_1234567890_deadbeefcafebabe" -> Some("blazing")
#[cfg(test)]
pub fn extract_server_name(server_id: &str) -> Option<&str> {
    if let Some(rest) = server_id.strip_prefix("server_")
        && let Some(pos) = rest.find('_')
    {
        return Some(&rest[..pos]);
    }
    None
}

/// Generate a memorable session name
/// Returns (full_id, short_name) where:
/// - full_id is the storage identifier like "session_fox_1234567890_deadbeefcafebabe"
/// - short_name is the memorable part like "fox"
pub fn new_memorable_session_id() -> (String, String) {
    let ts = Utc::now().timestamp_millis();
    let rand: u64 = rand::random();

    // Use the random value to pick a word
    let idx = (rand as usize) % SESSION_NAMES.len();
    let (word, _) = SESSION_NAMES[idx];

    let short_name = word.to_string();
    let full_id = format!("session_{}_{ts}_{rand:016x}", word);

    (full_id, short_name)
}

/// Try to extract the memorable name from a session ID
/// e.g., "session_fox_1234567890_deadbeefcafebabe" -> Some("fox")
pub fn extract_session_name(session_id: &str) -> Option<&str> {
    if let Some(rest) = session_id.strip_prefix("session_") {
        // Session names are the first token after the prefix.
        // This supports both old IDs (session_name_ts) and new IDs
        // with an added random suffix (session_name_ts_rand).
        if let Some(pos) = rest.find('_') {
            return Some(&rest[..pos]);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_memorable_session_id() {
        let (full_id, short_name) = new_memorable_session_id();

        // Full ID should start with "session_"
        assert!(full_id.starts_with("session_"));

        // Short name should be non-empty
        assert!(!short_name.is_empty());

        // Full ID should contain the short name
        assert!(full_id.contains(&short_name));

        // Short name should have a specific icon (not default)
        let icon = session_icon(&short_name);
        assert_ne!(
            icon, "💫",
            "Name '{}' should have a specific icon",
            short_name
        );
    }

    #[test]
    fn test_extract_session_name() {
        assert_eq!(extract_session_name("session_fox_1234567890"), Some("fox"));
        assert_eq!(
            extract_session_name("session_fox_1234567890_deadbeefcafebabe"),
            Some("fox")
        );
        assert_eq!(
            extract_session_name("session_blue-whale_1234567890"),
            Some("blue-whale")
        );
        assert_eq!(
            extract_session_name("session_blue-whale_1234567890_deadbeefcafebabe"),
            Some("blue-whale")
        );
        assert_eq!(
            extract_session_name("session_1234567890_9876543210"),
            Some("1234567890")
        );
        assert_eq!(extract_session_name("invalid"), None);
        assert_eq!(extract_session_name("session_"), None);
    }

    #[test]
    fn test_unique_session_ids() {
        let ids: std::collections::HashSet<String> =
            (0..512).map(|_| new_memorable_session_id().0).collect();
        assert_eq!(
            ids.len(),
            512,
            "session IDs should stay unique in tight bursts"
        );
    }

    #[test]
    fn test_all_names_have_icons() {
        for (name, expected_icon) in SESSION_NAMES {
            let icon = session_icon(name);
            assert_eq!(icon, *expected_icon, "Icon mismatch for '{}'", name);
            assert_ne!(icon, "💫", "Name '{}' should have a specific icon", name);
        }
    }

    #[test]
    fn test_new_memorable_server_id() {
        let (full_id, short_name) = new_memorable_server_id();

        // Full ID should start with "server_"
        assert!(full_id.starts_with("server_"));

        // Short name should be non-empty
        assert!(!short_name.is_empty());

        // Full ID should contain the short name
        assert!(full_id.contains(&short_name));

        // Short name should have a specific icon (not default)
        let icon = server_icon(&short_name);
        assert_ne!(
            icon, "🔮",
            "Modifier '{}' should have a specific icon",
            short_name
        );
    }

    #[test]
    fn test_extract_server_name() {
        assert_eq!(
            extract_server_name("server_blazing_1234567890"),
            Some("blazing")
        );
        assert_eq!(
            extract_server_name("server_blazing_1234567890_deadbeefcafebabe"),
            Some("blazing")
        );
        assert_eq!(
            extract_server_name("server_rising_1234567890"),
            Some("rising")
        );
        assert_eq!(extract_server_name("invalid"), None);
        assert_eq!(extract_server_name("server_"), None);
    }

    #[test]
    fn test_unique_server_ids() {
        let ids: std::collections::HashSet<String> =
            (0..256).map(|_| new_memorable_server_id().0).collect();
        assert_eq!(
            ids.len(),
            256,
            "server IDs should stay unique in tight bursts"
        );
    }

    #[test]
    fn test_all_modifiers_have_icons() {
        for (name, expected_icon) in SERVER_MODIFIERS {
            let icon = server_icon(name);
            assert_eq!(icon, *expected_icon, "Icon mismatch for '{}'", name);
            assert_ne!(
                icon, "🔮",
                "Modifier '{}' should have a specific icon",
                name
            );
        }
    }
}
