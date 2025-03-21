use std::fs::File;

/// Make sure that the config.yml we ship parses.
#[test]
fn test_config() {
    let file = File::open("config.yml").unwrap();
    let _ : qastor::config::Config = serde_yaml::from_reader(file).unwrap();
}