use super::{KillmailDetail, KillmailItem};
use std::collections::HashMap;

/// Estimates the loss from known market prices, or `None` when no price is known.
pub(super) fn estimate_killmail_value(
    detail: &KillmailDetail,
    market_prices: &HashMap<u64, f64>,
) -> Option<f64> {
    let ship = detail
        .victim
        .ship_type_id
        .and_then(|type_id| market_prices.get(&type_id).copied());
    add(ship, items_value(&detail.victim.items, market_prices))
}

fn items_value(items: &[KillmailItem], market_prices: &HashMap<u64, f64>) -> Option<f64> {
    items
        .iter()
        .map(|item| {
            let quantity = item.quantity_destroyed + item.quantity_dropped;
            let own = market_prices
                .get(&item.item_type_id)
                .map(|price| price * quantity as f64);
            add(own, items_value(&item.items, market_prices))
        })
        .fold(None, add)
}

fn add(left: Option<f64>, right: Option<f64>) -> Option<f64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left + right),
        (value, None) | (None, value) => value,
    }
}
