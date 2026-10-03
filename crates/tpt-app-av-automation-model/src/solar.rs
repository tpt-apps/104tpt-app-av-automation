//! Sunrise and sunset times for a site, computed offline (spec §7.1).
//!
//! A venue's exterior lighting should follow the sun, not a fixed clock: "house lights at sunset"
//! means something different every day, and hard-coding `18:00` means it is wrong twice a year. The
//! engine is offline-first (spec §1) with no timezone database and no network, so this module
//! computes the times itself from a latitude and longitude using the standard NOAA solar-position
//! algorithm, accurate to about a minute for the latitudes a venue can occupy.
//!
//! What it deliberately does not do is pretend to be more precise than it is. Dawn and dusk are
//! not instants: the sun is obscured by the horizon while refraction and the solar disc still put
//! light on the ground. A [`SolarSite`] therefore carries the elevation that defines the crossing,
//! and the default is the conventional -0.833 degrees (sunrise/sunset proper). A venue that wants
//! civil twilight picks that elevation itself.
//!
//! Accuracy notes: the algorithm takes no account of the observer's height above sea level, of
//! terrain, or of local horizon obstructions such as a neighbouring building. A site whose horizon
//! is blocked sees sunset earlier than computed; that is a site property no clock can know.

use serde::{Deserialize, Serialize};

/// The solar elevation, in degrees, that defines sunrise and sunset.
///
/// -0.833 is the conventional value: it accounts for mean atmospheric refraction at the horizon and
/// for the angular radius of the solar disc.
pub const SUNRISE_ELEVATION_DEG: f64 = -0.833;

/// Which solar crossing a time refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SolarEvent {
    /// The sun crosses the elevation going up.
    Sunrise,
    /// The sun crosses the elevation going down.
    Sunset,
}

impl std::fmt::Display for SolarEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl SolarEvent {
    /// The canonical label used in rule packs and reports.
    pub fn as_str(self) -> &'static str {
        match self {
            SolarEvent::Sunrise => "sunrise",
            SolarEvent::Sunset => "sunset",
        }
    }
}

/// A site's position on the Earth, and the elevation that defines its dawn and dusk.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SolarSite {
    /// Degrees north of the equator, -90 to 90.
    pub latitude: f64,
    /// Degrees east of Greenwich, -180 to 180.
    pub longitude: f64,
    /// Solar elevation that defines sunrise and sunset, in degrees. Defaults to the conventional
    /// sunrise/sunset value; set it to -6 for civil twilight.
    #[serde(default = "default_elevation")]
    pub elevation_deg: f64,
}

fn default_elevation() -> f64 {
    SUNRISE_ELEVATION_DEG
}

impl Default for SolarSite {
    fn default() -> Self {
        Self {
            latitude: 0.0,
            longitude: 0.0,
            elevation_deg: SUNRISE_ELEVATION_DEG,
        }
    }
}

impl SolarSite {
    /// A site at the given coordinates, using the conventional sunrise/sunset elevation.
    pub fn new(latitude: f64, longitude: f64) -> Self {
        Self {
            latitude,
            longitude,
            elevation_deg: SUNRISE_ELEVATION_DEG,
        }
    }

    /// Whether the coordinates and elevation are in range.
    pub fn is_valid(&self) -> bool {
        self.latitude.is_finite()
            && self.longitude.is_finite()
            && self.latitude.abs() <= 90.0
            && self.longitude.abs() <= 180.0
            && self.elevation_deg.is_finite()
            && self.elevation_deg.abs() <= 90.0
    }
}

/// Why a solar crossing could not be computed for a day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolarError {
    /// The sun never rises above the elevation on this day: polar night, or a high-latitude winter.
    NeverRises,
    /// The sun never sets below the elevation on this day: midnight sun, or a high-latitude summer.
    NeverSets,
}

/// The solar crossings for one calendar day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SolarDay {
    /// The sun rose and set.
    Normal {
        /// Minutes since local midnight.
        sunrise_minutes: u32,
        /// Minutes since local midnight.
        sunset_minutes: u32,
    },
    /// The sun did not cross the elevation on this day.
    Polar(PolarError),
}

const DEG: f64 = std::f64::consts::PI / 180.0;
const MINUTES_PER_DAY: i64 = 1440;

/// Days from 1970-01-01 to `year-month-day`, via Howard Hinnant's `days_from_civil`.
///
/// The inverse of [`civil_from_days`], so a date here lines up with the calendar the rest of the
/// engine works in.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The proleptic Gregorian date for a count of days since 1970-01-01.
pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Computes the solar crossings for one calendar day at one site.
///
/// `minutes_east_of_utc` converts the astronomical result into local wall-clock minutes, matching
/// the fixed-offset model the rest of the engine uses (spec §7.1).
///
/// Returns [`PolarError`] above the polar circles, where the crossing does not occur on that day.
/// That is not a failure to hide: a schedule that never fires at a polar venue is behaving
/// correctly, and the caller is told why rather than being handed a made-up time.
pub fn solar_day(
    site: &SolarSite,
    year: i64,
    month: i64,
    day: i64,
    minutes_east_of_utc: i32,
) -> Result<SolarDay, PolarError> {
    // NOAA's general solar position algorithm: day number, then the equation of time and the
    // solar declination, both of which shift the sun relative to the mean.
    let n = days_from_civil(year, month, day) as f64 + 2_440_587.5 - 2_451_545.0;
    let gamma = 2.0 * std::f64::consts::PI / 365.0 * (n - 1.0);
    let eqtime = 229.18
        * (0.000_075 + 0.001_868 * gamma.cos()
            - 0.032_077 * gamma.sin()
            - 0.014_615 * (2.0 * gamma).cos()
            - 0.040_849 * (2.0 * gamma).sin());
    let decl = 0.006_918 - 0.399_912 * gamma.cos() + 0.070_257 * gamma.sin()
        - 0.006_758 * (2.0 * gamma).cos()
        + 0.000_907 * (2.0 * gamma).sin()
        - 0.002_697 * (3.0 * gamma).cos()
        + 0.001_48 * (3.0 * gamma).sin();

    let lat = site.latitude * DEG;
    // `decl` comes out of the formula above already in radians, unlike the site's degrees.
    let dec = decl;
    let target = site.elevation_deg * DEG;

    // cos of the hour angle at which the sun sits exactly on the target elevation. Clamped, since
    // a site near a polar circle can produce a value a hair outside [-1, 1] from rounding.
    let cos_hour =
        ((target.sin() - lat.sin() * dec.sin()) / (lat.cos() * dec.cos())).clamp(-1.0, 1.0);

    if cos_hour.abs() >= 1.0 {
        // The sun never reaches the target elevation on this day, so there is no crossing to
        // schedule. Which polar case it is follows from where the sun sits at the two extremes of
        // the day: if it is above the target at midnight as well as at noon it never sets
        // (midnight sun); if it is below at noon it never rises (polar night).
        let altitude_at = |hour_angle: f64| -> f64 {
            (lat.sin() * dec.sin() + lat.cos() * dec.cos() * hour_angle.cos()).asin()
        };
        // If the sun is below the target even at local solar noon it never rises (polar night);
        // if it is still above the target at midnight it never sets (the midnight sun).
        if altitude_at(0.0) < target {
            return Err(PolarError::NeverRises);
        }
        return Err(PolarError::NeverSets);
    }

    let hour_angle = cos_hour.acos().to_degrees();
    // Local solar noon, shifted by longitude and then by the equation of time. Sunrise and sunset
    // sit either side of it by the hour angle, and the standard conversion is 4 minutes of clock
    // time per degree of hour angle.
    let noon = 720.0 - 4.0 * site.longitude + eqtime;
    let local_minutes = |rising: bool| -> i64 {
        let offset = if rising {
            -4.0 * hour_angle
        } else {
            4.0 * hour_angle
        };
        let minutes = noon + offset + f64::from(minutes_east_of_utc);
        minutes.round() as i64
    };

    Ok(SolarDay::Normal {
        sunrise_minutes: local_minutes(true).rem_euclid(MINUTES_PER_DAY) as u32,
        sunset_minutes: local_minutes(false).rem_euclid(MINUTES_PER_DAY) as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// London, 51.5 N.
    fn london() -> SolarSite {
        SolarSite::new(51.5074, -0.1278)
    }

    /// Sydney, 33.87 S.
    fn sydney() -> SolarSite {
        SolarSite::new(-33.8688, 151.2093)
    }

    /// Longyearbyen, 78.2 N, well inside the Arctic Circle.
    fn arctic() -> SolarSite {
        SolarSite::new(78.2232, 15.6469)
    }

    fn normal(
        site: &SolarSite,
        year: i64,
        month: i64,
        day: i64,
        offset_minutes: i32,
    ) -> (u32, u32) {
        match solar_day(site, year, month, day, offset_minutes) {
            Ok(SolarDay::Normal {
                sunrise_minutes,
                sunset_minutes,
            }) => (sunrise_minutes, sunset_minutes),
            other => panic!("expected a normal day, got {other:?}"),
        }
    }

    fn hours(minutes: u32) -> f64 {
        f64::from(minutes) / 60.0
    }

    #[test]
    fn a_midsummer_london_day_is_long_and_a_midwinter_one_short() {
        let (rise, set) = normal(&london(), 2024, 6, 21, 0);
        assert!(
            hours(set) - hours(rise) > 16.0,
            "the June day is over 16 hours long: {rise}..{set}"
        );
        let (rise, set) = normal(&london(), 2024, 12, 21, 0);
        assert!(
            hours(set) - hours(rise) < 8.5,
            "the December day is under 8.5 hours long: {rise}..{set}"
        );
    }

    #[test]
    fn the_southern_hemisphere_has_the_opposite_seasons() {
        let (sr, ss) = normal(&sydney(), 2024, 12, 21, 600);
        let (wr, ws) = normal(&sydney(), 2024, 6, 21, 600);
        assert!(
            hours(ss) - hours(sr) > hours(ws) - hours(wr),
            "December is the long day in Sydney"
        );
    }

    #[test]
    fn times_land_near_the_published_values_for_a_known_venue() {
        // London at BST (UTC+1) on the 2024 summer solstice: sunrise 04:43, sunset 21:21.
        let (rise, set) = normal(&london(), 2024, 6, 21, 60);
        assert!(
            (rise as i32 - 283).abs() <= 5,
            "sunrise {rise} min (04:43 = 283)"
        );
        assert!(
            (set as i32 - 1281).abs() <= 5,
            "sunset {set} min (21:21 = 1281)"
        );
    }

    #[test]
    fn a_site_further_east_sees_the_sun_earlier_by_the_longitude_difference() {
        // Two sites on the same latitude 15 degrees apart: one hour of daylight.
        let west = SolarSite::new(40.0, -15.0);
        let east = SolarSite::new(40.0, 0.0);
        let (wr, ws) = normal(&west, 2024, 3, 20, 0);
        let (er, es) = normal(&east, 2024, 3, 20, 0);
        assert!(
            (er as i32 - wr as i32).abs() >= 50,
            "sunrise differs by roughly the longitude difference: {wr} vs {er}"
        );
        assert!(
            (es as i32 - ws as i32).abs() >= 50,
            "so does sunset: {ws} vs {es}"
        );
    }

    #[test]
    fn the_equator_has_a_nearly_twelve_hour_day_all_year() {
        let equator = SolarSite::new(0.0, 0.0);
        for (month, day) in [(3, 20), (6, 21), (9, 22), (12, 21)] {
            let (rise, set) = normal(&equator, 2024, month, day, 0);
            let length = hours(set) - hours(rise);
            assert!(
                (length - 12.0).abs() < 0.5,
                "{month}/{day} should be near 12 hours, got {length}"
            );
        }
    }

    #[test]
    fn inside_the_arctic_circle_the_summer_day_has_no_sunset() {
        assert_eq!(
            solar_day(&arctic(), 2024, 6, 21, 0),
            Err(PolarError::NeverSets),
            "the midnight sun never sets"
        );
    }

    #[test]
    fn inside_the_arctic_circle_the_winter_day_has_no_sunrise() {
        assert_eq!(
            solar_day(&arctic(), 2024, 12, 21, 0),
            Err(PolarError::NeverRises),
            "polar night never rises"
        );
    }

    #[test]
    fn the_polar_seasons_flip_over_the_year() {
        let site = arctic();
        let summer = solar_day(&site, 2024, 6, 21, 0);
        let winter = solar_day(&site, 2024, 12, 21, 0);
        assert_eq!(summer, Err(PolarError::NeverSets));
        assert_eq!(winter, Err(PolarError::NeverRises));
    }

    #[test]
    // Sunrise/sunset is defined 0.83 degrees below the horizon, which puts the polar-night
    // boundary near 65.7 N on the solstice rather than at the geographic Arctic Circle, so
    // 65.0 N is comfortably outside it and still has both crossings.
    fn a_day_just_outside_the_circle_still_has_both_crossings() {
        let site = SolarSite::new(65.0, 25.0);
        assert!(
            solar_day(&site, 2024, 6, 21, 0).is_ok(),
            "65.0 N has both crossings on the solstice"
        );
    }

    #[test]
    fn the_arctic_boundary_is_where_the_polar_cases_begin() {
        let inside = SolarSite::new(66.6, 25.0);
        assert!(
            solar_day(&inside, 2024, 6, 21, 0).is_err(),
            "66.6 N is inside the Arctic Circle, so the solstice has no sunset"
        );
        // Sunrise/sunset is defined at a solar elevation of -0.833 degrees, so the polar-night
        // boundary sits near 90 - 23.44 - 0.83 = 65.7 N on the solstice, a little below the
        // geographic Arctic Circle at 66.56 N. 65.0 N is unambiguously outside it.
        let outside = SolarSite::new(65.0, 25.0);
        assert!(
            solar_day(&outside, 2024, 6, 21, 0).is_ok(),
            "65.0 N is still outside it"
        );
    }

    #[test]
    fn a_lower_elevation_gives_a_longer_day() {
        let mut twilight = london();
        // Civil twilight: the sun 6 degrees below the horizon.
        twilight.elevation_deg = -6.0;
        let (sunrise, sunset) = normal(&london(), 2024, 6, 21, 0);
        let (dusk, dawn) = normal(&twilight, 2024, 6, 21, 0);
        assert!(dusk < sunrise, "civil dawn starts before sunrise");
        assert!(dawn > sunset, "civil dusk ends after sunset");
    }

    #[test]
    fn every_day_of_the_year_produces_a_usable_result_at_a_venue_latitude() {
        // The sun rises and sets every day at a temperate latitude, which a venue relies on.
        for day_of_year in 0..365 {
            let (y, m, d) = civil_from_days(day_of_year);
            let (rise, set) = normal(&london(), y, m, d, 0);
            assert!(rise < 1440 && set < 1440);
            assert!(
                rise < set,
                "day {day_of_year} ({y}-{m}-{d}): sunrise {rise} must precede sunset {set}"
            );
        }
    }

    #[test]
    fn site_validation_catches_impossible_coordinates() {
        assert!(SolarSite::new(0.0, 0.0).is_valid());
        assert!(!SolarSite::new(91.0, 0.0).is_valid());
        assert!(!SolarSite::new(-91.0, 0.0).is_valid());
        assert!(!SolarSite::new(0.0, 181.0).is_valid());
        assert!(!SolarSite::new(f64::NAN, 0.0).is_valid());
        assert!(!SolarSite::new(f64::INFINITY, 0.0).is_valid());
        assert!(!SolarSite {
            elevation_deg: f64::NAN,
            ..london()
        }
        .is_valid());
    }

    #[test]
    fn extreme_coordinates_do_not_panic() {
        for (lat, lon) in [(90.0, 0.0), (-90.0, 0.0), (0.0, 180.0), (0.0, -180.0)] {
            let site = SolarSite::new(lat, lon);
            let _ = solar_day(&site, 2024, 6, 21, 0);
            let _ = solar_day(&site, 2024, 12, 21, 0);
        }
    }

    #[test]
    fn extreme_utc_offsets_stay_within_a_day() {
        for offset in [-720, -60, 0, 60, 720, 840, 1000] {
            let (rise, set) = normal(&london(), 2024, 6, 21, offset);
            assert!(rise < 1440 && set < 1440, "offset {offset}: {rise}..{set}");
        }
    }

    #[test]
    fn the_default_site_is_the_prime_meridian_at_the_equator() {
        let site = SolarSite::default();
        assert_eq!(site.latitude, 0.0);
        assert_eq!(site.longitude, 0.0);
        assert!((site.elevation_deg - SUNRISE_ELEVATION_DEG).abs() < f64::EPSILON);
        assert!(site.is_valid());
    }

    #[test]
    fn event_labels_are_stable() {
        assert_eq!(SolarEvent::Sunrise.as_str(), "sunrise");
        assert_eq!(SolarEvent::Sunset.as_str(), "sunset");
    }

    #[test]
    fn a_site_round_trips_through_yaml() {
        let yaml = "latitude: 51.5074\nlongitude: -0.1278\n";
        let site: SolarSite = serde_yaml::from_str(yaml).unwrap();
        assert!((site.latitude - 51.5074).abs() < 1e-9);
        assert!(
            (site.elevation_deg - SUNRISE_ELEVATION_DEG).abs() < 1e-12,
            "the elevation defaults when omitted"
        );
        let out = serde_yaml::to_string(&site).unwrap();
        assert_eq!(serde_yaml::from_str::<SolarSite>(&out).unwrap(), site);
    }

    #[test]
    fn civil_and_days_from_civil_are_inverses() {
        for days in [-100_000i64, -1, 0, 1, 19_723, 100_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(
                days_from_civil(y, m, d),
                days,
                "round trip failed at {days}"
            );
        }
    }
}
