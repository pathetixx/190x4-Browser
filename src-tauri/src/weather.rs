//! Погода на новой вкладке.
//!
//! Место — город, выбранный пользователем, или найденное автоматически:
//! сначала службой расположения Windows (по сетям Wi-Fi, поэтому прокси и VPN
//! на результат не влияют), а если она выключена — по IP-адресу. Название места
//! по координатам — из OpenStreetMap, прогноз и поиск городов — Open-Meteo.
//! Координаты перед отправкой округляются до сотых градуса, это около километра.

use std::time::Duration;

use anyhow::{bail, Context};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Ответ служб погоды и мест больше этого не бывает.
const RESPONSE_LIMIT: usize = 512 * 1024;

/// Службе названий мест нужен узнаваемый клиент, а не браузерная строка.
const APP_AGENT: &str = concat!(
    "190x4-browser/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/pathetixx/190x4-Browser)"
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub name: String,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub country: String,
    pub lat: f64,
    pub lon: f64,
}

impl Place {
    /// Проверка того, что пришло со страницы.
    pub fn is_valid(&self) -> bool {
        !self.name.trim().is_empty()
            && self.name.len() <= 120
            && self.region.len() <= 120
            && self.country.len() <= 120
            && (-90.0..=90.0).contains(&self.lat)
            && (-180.0..=180.0).contains(&self.lon)
    }
}

fn coordinate(value: f64) -> String {
    format!("{value:.2}")
}

async fn get_json(request: reqwest::RequestBuilder) -> anyhow::Result<Value> {
    let mut response = request.send().await?.error_for_status()?;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() > RESPONSE_LIMIT {
            bail!("слишком большой ответ");
        }
    }
    Ok(serde_json::from_slice(&body)?)
}

/// Города по названию.
pub async fn search(client: &reqwest::Client, query: &str) -> anyhow::Result<Vec<Place>> {
    let mut url = Url::parse("https://geocoding-api.open-meteo.com/v1/search")?;
    url.query_pairs_mut()
        .append_pair("name", query)
        .append_pair("count", "8")
        .append_pair("language", "ru")
        .append_pair("format", "json");
    let body = get_json(client.get(url)).await?;
    Ok(body["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(Place {
                name: item["name"].as_str()?.to_string(),
                region: item["admin1"].as_str().unwrap_or_default().to_string(),
                country: item["country"].as_str().unwrap_or_default().to_string(),
                lat: item["latitude"].as_f64()?,
                lon: item["longitude"].as_f64()?,
            })
        })
        .collect())
}

/// Сейчас, на сутки по часам и на пять дней.
pub async fn forecast(client: &reqwest::Client, place: &Place) -> anyhow::Result<Value> {
    let mut url = Url::parse("https://api.open-meteo.com/v1/forecast")?;
    url.query_pairs_mut()
        .append_pair("latitude", &coordinate(place.lat))
        .append_pair("longitude", &coordinate(place.lon))
        .append_pair(
            "current",
            "temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m,is_day",
        )
        .append_pair("hourly", "temperature_2m,precipitation_probability")
        .append_pair(
            "daily",
            "weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max",
        )
        .append_pair("forecast_days", "5")
        .append_pair("forecast_hours", "24")
        .append_pair("timezone", "auto")
        .append_pair("wind_speed_unit", "ms");
    let body = get_json(client.get(url)).await?;

    let current = &body["current"];
    if !current["temperature_2m"].is_number() {
        bail!("в прогнозе нет текущей погоды");
    }
    let daily = &body["daily"];
    let days = daily["time"].as_array().map_or(0, Vec::len);
    let hourly = &body["hourly"];
    let hours = hourly["time"].as_array().map_or(0, Vec::len);

    Ok(json!({
        "current": {
            "temp": current["temperature_2m"],
            "feels": current["apparent_temperature"],
            "humidity": current["relative_humidity_2m"],
            "code": current["weather_code"],
            "wind": current["wind_speed_10m"],
            "day": current["is_day"],
            "time": current["time"],
        },
        "hourly": (0..hours).map(|i| json!({
            "time": hourly["time"][i],
            "temp": hourly["temperature_2m"][i],
            "rain": hourly["precipitation_probability"][i],
        })).collect::<Vec<_>>(),
        "daily": (0..days).map(|i| json!({
            "date": daily["time"][i],
            "code": daily["weather_code"][i],
            "max": daily["temperature_2m_max"][i],
            "min": daily["temperature_2m_min"][i],
            "rain": daily["precipitation_probability_max"][i],
        })).collect::<Vec<_>>(),
    }))
}

/// Найти, где пользователь. Второе значение — `device` (служба расположения
/// Windows) или `network` (по IP-адресу, с прокси может ошибаться).
pub async fn locate(client: &reqwest::Client) -> anyhow::Result<(Place, &'static str)> {
    let device = tauri::async_runtime::spawn_blocking(device_position)
        .await
        .ok()
        .and_then(|result| {
            result
                .map_err(|err| tracing::debug!(%err, "служба расположения Windows не помогла"))
                .ok()
        });

    let (lat, lon, fallback, source) = match device {
        Some((lat, lon)) => (lat, lon, None, "device"),
        None => {
            let (lat, lon, city) = network_position(client).await?;
            (lat, lon, city, "network")
        }
    };

    let place = match reverse(client, lat, lon).await {
        Ok(place) => place,
        Err(err) => {
            tracing::debug!(%err, "название места не найдено");
            Place {
                name: fallback.unwrap_or_else(|| "Текущее место".to_string()),
                region: String::new(),
                country: String::new(),
                lat,
                lon,
            }
        }
    };
    Ok((place, source))
}

/// Координаты от службы расположения Windows. Служба может думать долго —
/// ждём не больше 12 секунд.
fn device_position() -> anyhow::Result<(f64, f64)> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> windows_core::Result<(f64, f64)> {
            use windows::Devices::Geolocation::Geolocator;
            use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

            unsafe {
                let _ = RoInitialize(RO_INIT_MULTITHREADED);
            }
            let locator = Geolocator::new()?;
            let position = locator.GetGeopositionAsync()?.get()?;
            let point = position.Coordinate()?.Point()?.Position()?;
            Ok((point.Latitude, point.Longitude))
        })();
        let _ = tx.send(result);
    });
    match rx.recv_timeout(Duration::from_secs(12)) {
        Ok(Ok(point)) => Ok(point),
        Ok(Err(err)) => bail!("служба расположения: {err}"),
        Err(_) => bail!("служба расположения не ответила"),
    }
}

async fn network_position(client: &reqwest::Client) -> anyhow::Result<(f64, f64, Option<String>)> {
    let body = get_json(client.get("https://get.geojs.io/v1/ip/geo.json")).await?;
    let number = |key: &str| {
        body[key]
            .as_str()
            .and_then(|text| text.parse::<f64>().ok())
            .or_else(|| body[key].as_f64())
    };
    let lat = number("latitude").context("нет широты")?;
    let lon = number("longitude").context("нет долготы")?;
    Ok((lat, lon, body["city"].as_str().map(str::to_string)))
}

async fn reverse(client: &reqwest::Client, lat: f64, lon: f64) -> anyhow::Result<Place> {
    let mut url = Url::parse("https://nominatim.openstreetmap.org/reverse")?;
    url.query_pairs_mut()
        .append_pair("format", "jsonv2")
        .append_pair("lat", &coordinate(lat))
        .append_pair("lon", &coordinate(lon))
        .append_pair("zoom", "10")
        .append_pair("accept-language", "ru");
    let body = get_json(
        client
            .get(url)
            .header(reqwest::header::USER_AGENT, APP_AGENT),
    )
    .await?;
    let address = &body["address"];
    let name = [
        "city",
        "town",
        "village",
        "municipality",
        "hamlet",
        "county",
        "state",
    ]
    .iter()
    .find_map(|key| address[key].as_str())
    .context("у места нет названия")?;
    Ok(Place {
        name: name.to_string(),
        region: address["state"].as_str().unwrap_or_default().to_string(),
        country: address["country"].as_str().unwrap_or_default().to_string(),
        lat,
        lon,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_are_rounded_before_leaving() {
        assert_eq!(coordinate(55.788_741), "55.79");
        assert_eq!(coordinate(-0.004), "-0.00");
    }

    #[test]
    fn place_from_page_is_checked() {
        let place = Place {
            name: "Казань".into(),
            region: "Татарстан".into(),
            country: "Россия".into(),
            lat: 55.79,
            lon: 49.12,
        };
        assert!(place.is_valid());
        assert!(!Place {
            lat: 91.0,
            ..place.clone()
        }
        .is_valid());
        assert!(!Place {
            name: " ".into(),
            ..place
        }
        .is_valid());
    }
}
