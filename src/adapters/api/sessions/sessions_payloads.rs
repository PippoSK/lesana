use serde::Deserialize;
#[derive(Deserialize)]
pub struct Login {
    #[serde(default)]
    pub password: String,
}
