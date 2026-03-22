use fe_database::RoleId;

pub struct Session {
    pub pub_key: [u8; 32],
    pub role: RoleId,
    pub token: String,
}
