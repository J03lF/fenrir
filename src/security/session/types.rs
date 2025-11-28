use time::OffsetDateTime;

use crate::security::auth::Role;

#[derive(Clone, Debug)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub role: Role,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub last_activity: OffsetDateTime,
}
