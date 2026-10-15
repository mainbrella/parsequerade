use parsequerade::{Client, Query};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let key = std::fs::read_to_string(std::env::var("ASC_PRIVATE_KEY_PATH")?)?;
    let client = Client::new(
        &std::env::var("ASC_ISSUER_ID")?,
        &std::env::var("ASC_KEY_ID")?,
        &key,
    )?;
    let query = Query::new().with("limit", 50);
    let mut cursor = None;
    loop {
        let mut page_query = query.clone();
        if let Some(value) = cursor {
            page_query = page_query.with("cursor", value);
        }
        let page = client.apps(&page_query).await?;
        for app in page.items {
            println!("{}\t{}", app.id, app.name.as_deref().unwrap_or("(unnamed)"));
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    Ok(())
}
