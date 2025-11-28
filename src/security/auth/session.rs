use super::role::Role;

#[derive(Debug, Clone)]
pub struct Session {
    pub user_id: String,
    pub role: Role,
}
