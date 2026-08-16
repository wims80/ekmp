use super::{Item, Killmail, KillmailItem, Victim};
use std::collections::HashMap;

pub(super) fn estimate_killmail_value(
    victim: &Victim,
    market_prices: &HashMap<u64, f64>,
) -> Option<f64> {
    let mut found_price = false;
    let mut value = victim
        .ship_type_id
        .and_then(|type_id| market_prices.get(&type_id))
        .map(|price| {
            found_price = true;
            *price
        })
        .unwrap_or_default();
    value += estimate_raw_items(&victim.items, market_prices, &mut found_price);
    found_price.then_some(value)
}

fn estimate_raw_items(
    items: &[Item],
    market_prices: &HashMap<u64, f64>,
    found_price: &mut bool,
) -> f64 {
    items
        .iter()
        .map(|item| {
            let quantity =
                item.quantity_destroyed.unwrap_or(0) + item.quantity_dropped.unwrap_or(0);
            let own_value = market_prices
                .get(&item.item_type_id)
                .map(|price| {
                    *found_price = true;
                    *price * quantity as f64
                })
                .unwrap_or_default();
            own_value + estimate_raw_items(&item.items, market_prices, found_price)
        })
        .sum()
}

pub(super) fn estimate_stored_killmail_value(
    mail: &Killmail,
    market_prices: &HashMap<u64, f64>,
) -> Option<f64> {
    let detail = mail.detail.as_ref()?;
    let mut found_price = false;
    let mut value = detail
        .victim
        .ship_type_id
        .and_then(|type_id| market_prices.get(&type_id))
        .map(|price| {
            found_price = true;
            *price
        })
        .unwrap_or_default();
    value += estimate_stored_items(&detail.victim.items, market_prices, &mut found_price);
    found_price.then_some(value)
}

fn estimate_stored_items(
    items: &[KillmailItem],
    market_prices: &HashMap<u64, f64>,
    found_price: &mut bool,
) -> f64 {
    items
        .iter()
        .map(|item| {
            let quantity = item.quantity_destroyed + item.quantity_dropped;
            let own_value = market_prices
                .get(&item.item_type_id)
                .map(|price| {
                    *found_price = true;
                    *price * quantity as f64
                })
                .unwrap_or_default();
            own_value + estimate_stored_items(&item.items, market_prices, found_price)
        })
        .sum()
}
