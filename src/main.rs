use std::env;

use scraper::{Html, Selector, ElementRef};
use chrono::NaiveDateTime;
use chrono::Local;
use chrono::naive::Days;
use telegram_bot::*;
use tokio_stream::StreamExt;
use tokio_cron_scheduler::{JobScheduler, Job};
use sqlite::State;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let	token = env::var("TELEGRAM_BOT_TOKEN").expect("TELEGRAM_BOT_TOKEN not set");
    let api = Api::new(token);
    let mut stream = api.stream();


    let connection = sqlite::open("./database").unwrap();

    let query = "
      CREATE TABLE IF NOT EXISTS chats (name TEXT, chat_id INTEGER);
    ";

    connection.execute(query).unwrap();


    let sched = JobScheduler::new().await?;

    let check_outages_job = Job::new_async("0 0 * * * *", |_uuid, _l| Box::pin(async move {
	println!("This will run job at 12:00 am every day");
        let token = env::var("TELEGRAM_BOT_TOKEN").expect("TELEGRAM_BOT_TOKEN not set");
        let api = Api::new(&token);
        let connection = sqlite::open("./database").unwrap();

	if let Some(info) = fetch_info().await.unwrap() {
	    let query = "SELECT name, chat_id FROM chats";
	    let mut statement = connection.prepare(query).unwrap();

	    while let Ok(State::Row) = statement.next() {
		let chat = ChatId::new(statement.read::<i64, _>("chat_id").unwrap());
		api.spawn(chat.text(&info));
	    }
	}
    })).unwrap();

    sched.add(check_outages_job).await?;

    sched.start().await?;


    while let Some(update) = stream.next().await {
        let update = update?;
        if let UpdateKind::Message(message) = update.kind {
            match message.kind {
                MessageKind::Text { ref data, .. } if data.as_str() == "/check" => {
                    let api = api.clone();
                    tokio::spawn(async move {
                        if let Err(err) = check_status(api, message).await {
                            eprintln!("Error: {:?}", err);
                        }
                    });
                }
                _ => (),
            };
        }
    }

    Ok(())
}

async fn check_status(api: Api, message: Message) -> Result<(), telegram_bot::Error> {
    let info = fetch_info().await.unwrap().unwrap_or("There is no planed power outage".to_string());
    api.send(message.text_reply(&info)).await?;

    Ok(())
}

async fn fetch_info() -> Result<Option<String>, Box<dyn std::error::Error>> {
    let resp = reqwest::get("https://pgedystrybucja.pl/planowanewylaczenia/wylaczenia/Legionowo").await?;
    let text = resp.text().await?;

    let document = Html::parse_document(&text);
    let selector = Selector::parse(r#"div > table > tbody > tr "#).unwrap();
    let selector_1 = Selector::parse(r#"td"#).unwrap();
    let selector_2 = Selector::parse(r#"ul > li"#).unwrap();

    for row in document.select(&selector).filter(|row| row.html().as_str().contains("Nieporęt")) {
	let mut inner_rows: scraper::element_ref::Select = row.select(&selector_1);

	let streets = inner_rows
	    .nth(0)
	    .unwrap()
	    .select(&selector_2)
	    .map(|tag| tag.inner_html())
	    .map(|street| format!("   - {}\n", street))
	    .collect::<String>();

	let now = Local::now().naive_local().checked_sub_days(Days::new(1)).unwrap();

	let power_outage_start = try_parse_date(inner_rows.next()).unwrap();
	let power_outage_end = try_parse_date(inner_rows.next()).unwrap();

	let is_before_start = power_outage_start.signed_duration_since(now).num_hours() >= 0;
	let is_before_end = power_outage_end.signed_duration_since(now).num_hours() >= 0;

	if is_before_start && is_before_end {
	    let message = format!(
		"WARN: Planned power outage in Nieporęt at {} to {}\n\n Affected addresses:\n{}",
		power_outage_start,
		power_outage_end,
		streets
	    );
            return Ok(Option::Some(message))
	}
    }
    Ok(Option::None)
}

fn try_parse_date(o_row: Option<ElementRef>) -> Option<NaiveDateTime> {
    o_row.map(|row| NaiveDateTime::parse_from_str(&row.inner_html(), "%Y-%m-%d %H:%M").unwrap())
}
