//! Weather for the chat, from Open-Meteo (no key needed).
//!
//! The tool returns a short written summary plus a ready-made `widget` block
//! (see widget.rs): today's conditions as a card with the week's forecast
//! behind an expand button. The agent loop makes sure the block reaches the
//! reply even if the model forgets to paste it.

use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::session::SessionInfo;

const GEOCODE_URL: &str = "https://geocoding-api.open-meteo.com/v1/search";
const FORECAST_URL: &str = "https://api.open-meteo.com/v1/forecast";
const FORECAST_DAYS: usize = 7;

#[derive(Deserialize)]
struct Args {
    /// A city name. Empty means the user's own location.
    #[serde(default)]
    location: String,
}

/// Weather code (WMO) to a description and an Iconify `meteocons` icon name.
fn describe_code(code: i64, day: bool) -> (&'static str, &'static str) {
    let sky = |d: &'static str, n: &'static str| if day { d } else { n };
    match code {
        0 => ("Clear sky", sky("clear-day-fill", "clear-night-fill")),
        1 => ("Mostly clear", sky("clear-day-fill", "clear-night-fill")),
        2 => ("Partly cloudy", sky("partly-cloudy-day-fill", "partly-cloudy-night-fill")),
        3 => ("Overcast", "overcast-fill"),
        45 | 48 => ("Fog", "fog-fill"),
        51 | 53 | 55 | 56 | 57 => ("Drizzle", "drizzle-fill"),
        61 | 63 | 80 | 81 => ("Rain", sky("partly-cloudy-day-rain-fill", "partly-cloudy-night-rain-fill")),
        65 | 82 => ("Heavy rain", "rain-fill"),
        66 | 67 => ("Freezing rain", "sleet-fill"),
        71 | 73 | 75 | 77 | 85 | 86 => ("Snow", "snow-fill"),
        95 => ("Thunderstorm", "thunderstorms-fill"),
        96 | 99 => ("Thunderstorm with hail", "hail-fill"),
        _ => ("Cloudy", "cloudy-fill"),
    }
}

async fn get_json(url: &str, query: &[(&str, String)]) -> Result<Value, String> {
    let resp = crate::openrouter::client()
        .get(url)
        .query(query)
        .send()
        .await
        .map_err(|e| format!("Couldn't reach the weather service: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("The weather service answered {}.", resp.status()));
    }
    resp.json().await.map_err(|e| format!("Couldn't read the weather: {e}"))
}

fn day_label(date: &str, index: usize) -> String {
    if index == 0 {
        return "Today".to_string();
    }
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|d| d.format("%a %-d %b").to_string())
        .unwrap_or_else(|_| date.to_string())
}

pub async fn get(app: &AppHandle, args: &str) -> Result<String, String> {
    let Args { location } = serde_json::from_str(args).map_err(|e| format!("Bad arguments: {e}"))?;
    let mut wanted = location.trim().to_string();
    if wanted.is_empty() {
        let place = app.state::<SessionInfo>().location().await?;
        wanted = place.city;
    }
    // The geocoder searches by name only: "Delhi, India" must become "Delhi".
    let name = wanted.split(',').next().unwrap_or("").trim().to_string();
    if name.is_empty() {
        return Err("No place given and the user's location isn't known.".to_string());
    }

    let found = get_json(GEOCODE_URL, &[("name", name.clone()), ("count", "1".into()), ("language", "en".into())]).await?;
    let hit = found["results"].get(0).ok_or_else(|| format!("No place found called \"{name}\"."))?;
    let (lat, lon) = (hit["latitude"].as_f64().unwrap_or(0.0), hit["longitude"].as_f64().unwrap_or(0.0));
    let place = [hit["name"].as_str(), hit["country"].as_str()].into_iter().flatten().collect::<Vec<_>>().join(", ");

    let data = get_json(
        FORECAST_URL,
        &[
            ("latitude", lat.to_string()),
            ("longitude", lon.to_string()),
            ("current", "temperature_2m,apparent_temperature,relative_humidity_2m,wind_speed_10m,weather_code,is_day".into()),
            ("daily", "weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max".into()),
            ("forecast_days", FORECAST_DAYS.to_string()),
            ("timezone", "auto".into()),
        ],
    )
    .await?;

    let cur = &data["current"];
    let daily = &data["daily"];
    let num = |v: &Value| v.as_f64().map(|n| n.round() as i64);
    let temp = num(&cur["temperature_2m"]).ok_or("The weather service sent no current temperature.")?;
    let day = cur["is_day"].as_i64().unwrap_or(1) == 1;
    let code = cur["weather_code"].as_i64().unwrap_or(3);
    let (condition, icon) = describe_code(code, day);
    let high = num(&daily["temperature_2m_max"][0]);
    let low = num(&daily["temperature_2m_min"][0]);
    let rain = num(&daily["precipitation_probability_max"][0]);
    let feels = num(&cur["apparent_temperature"]);
    let humidity = num(&cur["relative_humidity_2m"]);
    let wind = num(&cur["wind_speed_10m"]);

    let mut chips = Vec::new();
    if let (Some(h), Some(l)) = (high, low) {
        chips.push(json!({ "icon": "lucide:thermometer", "text": format!("{h}° / {l}°") }));
    }
    if let Some(r) = rain {
        chips.push(json!({ "icon": "lucide:umbrella", "text": format!("{r}% rain") }));
    }
    if let Some(h) = humidity {
        chips.push(json!({ "icon": "lucide:droplets", "text": format!("{h}% humidity") }));
    }
    if let Some(w) = wind {
        chips.push(json!({ "icon": "lucide:wind", "text": format!("{w} km/h") }));
    }

    let mut rows = Vec::new();
    let (mut labels, mut highs, mut lows) = (Vec::new(), Vec::new(), Vec::new());
    let mut week = String::new();
    for i in 0..FORECAST_DAYS {
        let Some(date) = daily["time"][i].as_str() else { break };
        let (Some(h), Some(l)) = (num(&daily["temperature_2m_max"][i]), num(&daily["temperature_2m_min"][i])) else { continue };
        let (text, row_icon) = describe_code(daily["weather_code"][i].as_i64().unwrap_or(3), true);
        let label = day_label(date, i);
        let rain = num(&daily["precipitation_probability_max"][i]);
        let note = match rain {
            Some(r) if r >= 20 => format!("{text}, {r}% rain"),
            _ => text.to_string(),
        };
        labels.push(NaiveDate::parse_from_str(date, "%Y-%m-%d").map(|d| d.format("%a").to_string()).unwrap_or_else(|_| label.clone()));
        highs.push(h);
        lows.push(l);
        week.push_str(&format!(" {label}: {h}°/{l}°C, {note}."));
        rows.push(json!({ "icon": format!("meteocons:{row_icon}"), "label": label, "value": format!("{h}° / {l}°"), "note": note }));
    }

    let mut widget = json!({
        "title": place,
        "subtitle": "Now",
        "icon": format!("meteocons:{icon}"),
        "value": format!("{temp}°"),
        "caption": match feels { Some(f) => format!("{condition} · feels {f}°"), None => condition.to_string() },
        "chips": chips,
    });
    if !rows.is_empty() {
        widget["expandLabel"] = json!(format!("{}-day forecast", rows.len()));
        widget["rows"] = json!(rows);
    }

    let chart = if labels.len() > 1 {
        let spec = json!({
            "type": "line", "title": "Temperature this week", "unit": "°C", "labels": labels,
            "series": [{ "name": "High", "values": highs }, { "name": "Low", "values": lows }],
        });
        format!("

```chart
{spec}
```")
    } else {
        String::new()
    };

    let mut text = format!("Weather in {place}: {temp}°C, {condition}.");
    if let (Some(h), Some(l)) = (high, low) {
        text.push_str(&format!(" Today's high {h}°C, low {l}°C."));
    }
    if let Some(r) = rain {
        text.push_str(&format!(" Chance of rain today {r}%."));
    }
    if !week.is_empty() {
        text.push_str(&format!("\nForecast:{week}"));
    }
    Ok(format!(
        "{text}\n\nThe widget below shows all of this (current conditions and the forecast behind an expand button). Start your reply with the block(s) exactly as written, then write only extra detail below it: what to wear or bring, how the day goes. Do not repeat the numbers it shows.\n```widget\n{widget}\n```{chart}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_map_to_icons_and_respect_night() {
        assert_eq!(describe_code(0, true).1, "clear-day-fill");
        assert_eq!(describe_code(0, false).1, "clear-night-fill");
        assert_eq!(describe_code(95, true).0, "Thunderstorm");
        assert_eq!(describe_code(999, true).1, "cloudy-fill");
    }

    #[test]
    fn days_are_labelled() {
        assert_eq!(day_label("2026-10-19", 0), "Today");
        assert_eq!(day_label("2026-10-19", 1), "Mon 19 Oct");
    }
}
