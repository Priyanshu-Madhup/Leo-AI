//! The user's profile: name, date of birth, location, occupation, interests.
//!
//! Stored in the `profile` table of the local SQLite file (history.rs). The
//! table exists for everyone after an update but starts empty, so existing
//! users have no profile until they fill it in (the app asks them once).
//!
//! What it is used for:
//! - greetings on the empty chat screen (frontend, uses name and birthday);
//! - the agents' prompts (session.rs): name, location, occupation, interests,
//!   so replies can be personal. The **date of birth never leaves the device**:
//!   it is only used locally for the birthday greeting;
//! - the location, when given, replaces the IP-based lookup.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::history::Db;
use crate::session::SessionInfo;

const MAX_NAME: usize = 60;
const MAX_LOCATION: usize = 100;
const MAX_OCCUPATION: usize = 80;
const MAX_INTERESTS: usize = 300;

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
pub struct Profile {
    #[serde(default)]
    pub name: String,
    /// `YYYY-MM-DD`, or empty.
    #[serde(default)]
    pub dob: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub occupation: String,
    #[serde(default)]
    pub interests: String,
}

/// One line of text: control characters and runs of whitespace become single
/// spaces, then it is cut to `max` characters.
fn tidy(text: &str, max: usize) -> String {
    let cleaned: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

impl Profile {
    pub fn is_empty(&self) -> bool {
        [&self.name, &self.dob, &self.location, &self.occupation, &self.interests]
            .iter()
            .all(|f| f.trim().is_empty())
    }

    /// Trimmed, length-limited and checked; an invalid date of birth is an error.
    pub fn cleaned(&self) -> Result<Profile, String> {
        let dob = self.dob.trim().to_string();
        if !dob.is_empty() {
            let date = chrono::NaiveDate::parse_from_str(&dob, "%Y-%m-%d")
                .map_err(|_| "The date of birth is not a valid date.".to_string())?;
            let earliest = chrono::NaiveDate::from_ymd_opt(1900, 1, 1).unwrap();
            if date < earliest || date > chrono::Local::now().date_naive() {
                return Err("The date of birth looks wrong.".to_string());
            }
        }
        Ok(Profile {
            name: tidy(&self.name, MAX_NAME),
            dob,
            location: tidy(&self.location, MAX_LOCATION),
            occupation: tidy(&self.occupation, MAX_OCCUPATION),
            interests: tidy(&self.interests, MAX_INTERESTS),
        })
    }
}

/// Makes sure the agents know the saved profile even if the screen has not
/// asked for it yet (it normally does at start-up).
pub fn ensure_loaded(app: &AppHandle) {
    let session = app.state::<SessionInfo>();
    if session.take_unloaded_flag() {
        match app.state::<Db>().get_profile(app) {
            Ok(profile) => session.set_profile(&profile),
            Err(err) => eprintln!("could not read the profile: {err}"),
        }
    }
}

/// The saved profile (empty for users who never filled it in). Also hands it
/// to the agents.
#[tauri::command]
pub fn profile_get(app: AppHandle, db: State<'_, Db>, session: State<'_, SessionInfo>) -> Result<Profile, String> {
    let profile = db.get_profile(&app)?;
    session.set_profile(&profile);
    Ok(profile)
}

#[tauri::command]
pub fn profile_save(
    app: AppHandle,
    db: State<'_, Db>,
    session: State<'_, SessionInfo>,
    profile: Profile,
) -> Result<Profile, String> {
    let profile = profile.cleaned()?;
    db.save_profile(&app, &profile)?;
    session.set_profile(&profile);
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_tidied_and_limited() {
        let p = Profile {
            name: "  Priyanshu \n Madhup ".into(),
            dob: "2001-05-12".into(),
            location: "x".repeat(500),
            occupation: "".into(),
            interests: "chess,\tf1\r\ncode".into(),
        };
        let c = p.cleaned().unwrap();
        assert_eq!(c.name, "Priyanshu Madhup");
        assert_eq!(c.location.chars().count(), MAX_LOCATION);
        assert_eq!(c.interests, "chess, f1 code");
    }

    #[test]
    fn bad_birth_dates_are_refused() {
        let with = |d: &str| Profile { dob: d.into(), ..Default::default() }.cleaned();
        assert!(with("").is_ok());
        assert!(with("2001-05-12").is_ok());
        assert!(with("12/05/2001").is_err());
        assert!(with("2001-02-30").is_err());
        assert!(with("1800-01-01").is_err());
        assert!(with("2999-01-01").is_err());
    }

    #[test]
    fn empty_means_nothing_filled_in() {
        assert!(Profile::default().is_empty());
        assert!(Profile { name: "  ".into(), ..Default::default() }.is_empty());
        assert!(!Profile { location: "Delhi".into(), ..Default::default() }.is_empty());
    }
}
