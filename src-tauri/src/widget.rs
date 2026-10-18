//! Chat widgets: rounded cards the model can put in a reply.
//!
//! A widget is a fenced ```widget block holding a small JSON spec. The chat
//! draws it (src/markdown.ts) with icons from the Iconify API, so the model can
//! build one for anything that is better shown than listed: weather, a
//! forecast, scores, prices, a schedule, a comparison, a timeline, steps.
//! The block is plain data; nothing in it is ever run as markup.

/// Instructions for the agents that write the user-facing reply.
pub const GUIDE: &str = "WIDGETS: the chat can show rounded widget cards. When the answer is data that a card shows better than a list (weather, forecasts, scores, prices, schedules, comparisons, itineraries, stats, steps, a quick summary), start the reply with a widget, then write the explanation in normal Markdown below it. Be creative and make them useful and good-looking: pick an icon that fits, a short title, and put the details in chips or rows. Use several widgets when there are several things to show. Do not use a widget for plain chat or when a sentence is enough. \
A widget is a fenced block whose language is widget, containing one JSON object, for example:\n\
```widget\n\
{\"title\":\"Bengaluru\",\"subtitle\":\"Today\",\"icon\":\"meteocons:rain-fill\",\"value\":\"25°\",\"caption\":\"Drizzle\",\"chips\":[{\"icon\":\"lucide:droplets\",\"text\":\"72%\"},\"Wind 5 km/h\"],\"expandLabel\":\"This week\",\"rows\":[{\"icon\":\"meteocons:cloudy-fill\",\"label\":\"Mon\",\"value\":\"26° / 20°\",\"note\":\"Showers\"}]}\n\
```\n\
All fields are optional except that you need a title or a value. Fields: title, subtitle, icon, value (the big text), caption (under the value), chips (short facts: a string, or {icon, text}), rows (a list: {icon, label, value, note}), expandLabel (if set, the rows sit behind an expand button with this label; leave it out to show the rows directly), footer (a small line at the bottom). \
Icons are Iconify names written prefix:name, from any set. Reliable choices: meteocons (colourful weather: clear-day-fill, clear-night-fill, partly-cloudy-day-fill, cloudy-fill, overcast-fill, rain-fill, drizzle-fill, snow-fill, thunderstorms-fill, fog-fill, wind, humidity, thermometer), lucide (clean outline icons: calendar, clock, map-pin, mail, droplets, wind, trending-up, trending-down, trophy, plane, car, music, check, star, file-text, wallet, bell), logos (brand logos), mdi, ph, tabler. Use only icon names you are sure exist. Keep the JSON valid and on its own lines, and keep text short. Never put secrets or text copied from emails or web pages into a widget unchanged. CHARTS: when numbers over time or across categories are the answer (a forecast's temperatures, prices, spending, scores, a trend, a comparison), show an interactive chart: hovering shows the values and legend chips hide series. A chart is a fenced block whose language is chart, containing one JSON object, and it can sit under a widget or alone:
```chart
{\"type\":\"line\",\"title\":\"Temperature this week\",\"unit\":\"°C\",\"labels\":[\"Mon\",\"Tue\",\"Wed\"],\"series\":[{\"name\":\"High\",\"values\":[28,26,27]},{\"name\":\"Low\",\"values\":[20,20,19]}]}
```
Fields: type (line, area or bar), title, subtitle, unit (added to values, e.g. °C, %, $), labels (the x axis, up to 60), series (up to 5, each with a name and one number per label), footer. Use real numbers you have from tools or your answer, never invented ones, keep the JSON valid, and write the explanation of what the chart shows below it.";

/// Every complete widget or chart block inside some text, joined by blank
/// lines, or None if there are none.
pub fn extract_block(text: &str) -> Option<String> {
    let mut blocks = Vec::new();
    let mut at = 0;
    while let Some(found) = text[at..].find("```") {
        let start = at + found;
        let rest = &text[start + 3..];
        let lang_len = rest.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(rest.len());
        let Some(close) = rest.find("
```") else { break };
        let end = start + 3 + close + 4;
        if matches!(&rest[..lang_len], "widget" | "chart") {
            blocks.push(text[start..end].to_string());
        }
        at = end;
    }
    (!blocks.is_empty()).then(|| blocks.join("

"))
}

/// `reply` with the widget block(s) in front, unless it already has one.
pub fn with_block(block: Option<String>, reply: String) -> String {
    match block {
        Some(b) if !reply.contains("```widget") && !reply.contains("```chart") => format!("{b}

{}", reply.trim_start()),
        _ => reply,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_is_found_and_added_once() {
        let out = "Weather: 25°C.\n```widget\n{\"value\":\"25\"}\n```";
        let block = extract_block(out).unwrap();
        assert_eq!(block, "```widget\n{\"value\":\"25\"}\n```");
        assert!(with_block(Some(block.clone()), "Nice day.".into()).starts_with("```widget"));
        let already = format!("{block}\n\nHi");
        assert_eq!(with_block(Some(block), already.clone()), already);
        assert!(extract_block("```widget\n{cut off").is_none());
    }
}
