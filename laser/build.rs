fn main() {
    // Only the esp-idf (std) build needs the ESP-IDF linker/env propagation.
    if std::env::var_os("CARGO_FEATURE_IDF").is_some() {
        embuild::espidf::sysenv::output();
    }
}
