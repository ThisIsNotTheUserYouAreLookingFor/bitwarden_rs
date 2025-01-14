use diesel;
use diesel::prelude::*;
use serde_json::Value;

use crate::api::EmptyResult;
use crate::db::schema::twofactor;
use crate::db::DbConn;
use crate::error::MapResult;

use super::User;

#[derive(Debug, Identifiable, Queryable, Insertable, Associations)]
#[table_name = "twofactor"]
#[belongs_to(User, foreign_key = "user_uuid")]
#[primary_key(uuid)]
pub struct TwoFactor {
    pub uuid: String,
    pub user_uuid: String,
    pub atype: i32,
    pub enabled: bool,
    pub data: String,
}

#[allow(dead_code)]
#[derive(FromPrimitive)]
pub enum TwoFactorType {
    Authenticator = 0,
    Email = 1,
    Duo = 2,
    YubiKey = 3,
    U2f = 4,
    Remember = 5,
    OrganizationDuo = 6,

    // These are implementation details
    U2fRegisterChallenge = 1000,
    U2fLoginChallenge = 1001,
    EmailVerificationChallenge = 1002,
}

/// Local methods
impl TwoFactor {
    pub fn new(user_uuid: String, atype: TwoFactorType, data: String) -> Self {
        Self {
            uuid: crate::util::get_uuid(),
            user_uuid,
            atype: atype as i32,
            enabled: true,
            data,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "Enabled": self.enabled,
            "Key": "", // This key and value vary
            "Object": "twoFactorAuthenticator" // This value varies
        })
    }

    pub fn to_json_list(&self) -> Value {
        json!({
            "Enabled": self.enabled,
            "Type": self.atype,
            "Object": "twoFactorProvider"
        })
    }
}

/// Database methods
impl TwoFactor {
    pub fn save(&self, conn: &DbConn) -> EmptyResult {
        diesel::replace_into(twofactor::table)
            .values(self)
            .execute(&**conn)
            .map_res("Error saving twofactor")
    }

    pub fn delete(self, conn: &DbConn) -> EmptyResult {
        diesel::delete(twofactor::table.filter(twofactor::uuid.eq(self.uuid)))
            .execute(&**conn)
            .map_res("Error deleting twofactor")
    }

    pub fn find_by_user(user_uuid: &str, conn: &DbConn) -> Vec<Self> {
        twofactor::table
            .filter(twofactor::user_uuid.eq(user_uuid))
            .filter(twofactor::atype.lt(1000)) // Filter implementation types
            .load::<Self>(&**conn)
            .expect("Error loading twofactor")
    }

    pub fn find_by_user_and_type(user_uuid: &str, atype: i32, conn: &DbConn) -> Option<Self> {
        twofactor::table
            .filter(twofactor::user_uuid.eq(user_uuid))
            .filter(twofactor::atype.eq(atype))
            .first::<Self>(&**conn)
            .ok()
    }

    pub fn delete_all_by_user(user_uuid: &str, conn: &DbConn) -> EmptyResult {
        diesel::delete(twofactor::table.filter(twofactor::user_uuid.eq(user_uuid)))
            .execute(&**conn)
            .map_res("Error deleting twofactors")
    }
}
