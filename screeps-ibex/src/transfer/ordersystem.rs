use super::fairvalue::*;
use super::utility::*;
use crate::memorysystem::MemoryArbiter;
use crate::missions::constants::*;
use crate::room::data::*;
use crate::segments::MARKET_SEGMENT;
use log::*;
use screeps::game::market::*;
use screeps::*;
use specs::prelude::{Entities, LazyUpdate, Read, ResourceId, System, SystemData, World, Write, WriteExpect, WriteStorage};
use std::collections::{BTreeMap, HashMap};

/// Passive orders are placed (and active deals sized) in blocks of this many
/// units — the per-order damage cap that predates ADR 0012 and survives it.
const TRADE_BLOCK_UNITS: u32 = 2000;

pub struct OrderQueuePassiveRequest {
    resource: ResourceType,
    amount: u32,
}

pub struct OrderQueueActiveRequest {
    resource: ResourceType,
    amount: u32,
    available_transfer_energy: u32,
}

pub struct OrderQueueRoomData {
    outgoing_passive_requests: Vec<OrderQueuePassiveRequest>,
    outgoing_active_requests: Vec<OrderQueueActiveRequest>,

    incoming_passive_requests: Vec<OrderQueuePassiveRequest>,
}

impl OrderQueueRoomData {
    pub fn new() -> OrderQueueRoomData {
        OrderQueueRoomData {
            outgoing_passive_requests: Vec::new(),
            outgoing_active_requests: Vec::new(),

            incoming_passive_requests: Vec::new(),
        }
    }
}

#[derive(Default)]
pub struct OrderQueue {
    // BTreeMap, not HashMap: iteration order arbitrates shared finite
    // resources (credits, the external order book, exposure window caps),
    // so it must be deterministic (EP-6.13).
    rooms: BTreeMap<RoomName, OrderQueueRoomData>,
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl OrderQueue {
    pub fn maximum_transfer_energy() -> u32 {
        10000
    }

    pub fn get_room(&mut self, room: RoomName) -> &mut OrderQueueRoomData {
        self.rooms.entry(room).or_insert_with(OrderQueueRoomData::new)
    }

    pub fn request_passive_sale(&mut self, room: RoomName, resource: ResourceType, amount: u32) {
        let room = self.get_room(room);

        room.outgoing_passive_requests.push(OrderQueuePassiveRequest { resource, amount });
    }

    pub fn request_active_sale(&mut self, room: RoomName, resource: ResourceType, amount: u32, available_transfer_energy: u32) {
        let room = self.get_room(room);

        room.outgoing_active_requests.push(OrderQueueActiveRequest {
            resource,
            amount,
            available_transfer_energy,
        });
    }

    pub fn request_passive_purchase(&mut self, room: RoomName, resource: ResourceType, amount: u32) {
        let room = self.get_room(room);

        room.incoming_passive_requests.push(OrderQueuePassiveRequest { resource, amount });
    }

    pub fn clear(&mut self) {
        self.rooms.clear();
    }
}

#[derive(SystemData)]
pub struct OrderQueueSystemData<'a> {
    order_queue: Write<'a, OrderQueue>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    room_data: WriteStorage<'a, RoomData>,
    governor: Read<'a, crate::cpugovernor::GovernorSnapshot>,
    features: Read<'a, crate::features::Features>,
    market_memory: Write<'a, MarketMemory>,
    memory_arbiter: WriteExpect<'a, MemoryArbiter>,
}

/// Decode the market segment into a [`MarketMemory`], loudly starting fresh
/// on an empty segment, a version mismatch, or a decode failure. The block
/// carries its own version precisely so it never depends on (or risks) the
/// component segments' `WORLD_FORMAT_VERSION`.
fn load_market_memory(arbiter: &MemoryArbiter) -> MarketMemory {
    let mut memory = arbiter
        .get(MARKET_SEGMENT)
        .filter(|raw| !raw.is_empty())
        .and_then(|raw| match crate::serialize::decode_from_string::<MarketMemory>(&raw) {
            Ok(decoded) if decoded.version == MARKET_MEMORY_VERSION => Some(decoded),
            Ok(decoded) => {
                warn!(
                    "Market memory segment version {} (expected {}) - starting fresh",
                    decoded.version, MARKET_MEMORY_VERSION
                );
                None
            }
            Err(err) => {
                warn!("Failed to decode market memory segment - starting fresh: {}", err);
                None
            }
        })
        .unwrap_or_default();

    memory.loaded = true;
    memory
}

fn save_market_memory(arbiter: &mut MemoryArbiter, memory: &MarketMemory) {
    match crate::serialize::encode_to_string(memory) {
        Ok(encoded) => {
            // The market segment is not in the always-active set (which is
            // 10 of 10 in steady state), so the write goes through the
            // arbiter's queued-write path: it lands as soon as the shared
            // touch budget allows — usually the next tick, via a one-tick
            // reservation that displaces the lowest-priority active id.
            arbiter.queue_write(MARKET_SEGMENT, encoded);
        }
        Err(err) => warn!("Failed to encode market memory segment: {}", err),
    }
}

/// Fetch `get_history` once per resource per pass, merge it into the
/// persisted per-resource window, then run the pure FairValue oracle
/// (ADR 0012 §1 / IBEX-018). The JS records are copied into plain
/// `HistoryDay`s so the oracle itself stays host-testable.
fn cached_fair_value(
    cache: &mut HashMap<MarketResourceType, Option<FairValue>>,
    memory: &mut MarketMemory,
    resource: ResourceType,
) -> Option<FairValue> {
    *cache.entry(MarketResourceType::Resource(resource)).or_insert_with(|| {
        let fetched: Vec<HistoryDay> = game::market::get_history(Some(resource))
            .iter()
            .map(|record| HistoryDay {
                date: record.date_str(),
                avg_price: record.avg_price(),
                stddev_price: record.stddev_price(),
                volume: record.volume(),
                transactions: record.transactions(),
            })
            .collect();

        let days = memory.merge_history(resource, &fetched);
        fair_value(days, TRADE_BLOCK_UNITS as f64)
    })
}

struct PassiveOrderParameters {
    room_name: RoomName,
    resource: ResourceType,
    amount: u32,
    minimum_amount: u32,
    price: f64,
}

struct ActiveSellOrderParameters {
    resource: ResourceType,
    amount: u32,
    minimum_sale_amount: u32,
    minimum_price: f64,
    maximum_transfer_energy: u32,
    available_transfer_energy: u32,
    energy_cost: f64,
}

pub struct OrderQueueSystem;

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl OrderQueueSystem {
    fn sell_passive_order(my_orders: &JsHashMap<String, MyOrder>, exposure: &mut ExposureLedger, params: PassiveOrderParameters) {
        if params.amount < params.minimum_amount {
            //TODO: Handle order in progress, cancel etc.?
            return;
        }

        let market_resource_type = MarketResourceType::Resource(params.resource);

        let mut current_orders = my_orders
            .values()
            .filter(|o| o.order_type() == OrderType::Sell && o.resource_type() == market_resource_type)
            .filter(|o| o.remaining_amount() > 0)
            .filter(|o| {
                o.room_name()
                    .and_then(|order_room_name| order_room_name.as_string())
                    .map(|order_room_name| order_room_name == params.room_name)
                    .unwrap_or(false)
            });

        //
        // NOTE: Sell in block of minimum sale amount, not total capacity.
        //

        if current_orders.next().is_none() {
            let sell_amount = params.minimum_amount;

            // Per-window per-resource exposure cap (ADR 0012 §2).
            if !exposure.volume_within_cap(params.resource, sell_amount) {
                return;
            }

            let order_params = CreateOrderParams::new(
                OrderType::Sell,
                market_resource_type,
                params.price,
                sell_amount,
                Some(params.room_name),
            );

            match create_order(&order_params) {
                Ok(()) => {
                    exposure.commit_volume(params.resource, sell_amount);
                    info!(
                        "Placed sell order! Room: {} Resource: {:?} Price: {} Amount: {}",
                        params.room_name, params.resource, params.price, sell_amount
                    );
                }
                Err(err) => {
                    info!(
                        "Failed to place sell order! Error: {:?} Room: {} Resource: {:?} Price: {} Amount: {}",
                        err, params.room_name, params.resource, params.price, sell_amount
                    );
                }
            }
        }
    }

    fn buy_passive_order(
        my_orders: &JsHashMap<String, MyOrder>,
        exposure: &mut ExposureLedger,
        credits: f64,
        credit_reserve: f64,
        params: PassiveOrderParameters,
    ) {
        if params.amount < params.minimum_amount {
            //TODO: Handle order in progress, cancel etc.?
            return;
        }

        let market_resource_type = MarketResourceType::Resource(params.resource);

        let mut current_orders = my_orders
            .values()
            .filter(|o| o.order_type() == OrderType::Buy && o.resource_type() == market_resource_type)
            .filter(|o| o.remaining_amount() > 0)
            .filter(|o| {
                o.room_name()
                    .and_then(|order_room_name| order_room_name.as_string())
                    .map(|order_room_name| order_room_name == params.room_name)
                    .unwrap_or(false)
            });

        //
        // NOTE: Buy at maximum in blocks of minimum amount, not total desired.
        //

        if current_orders.next().is_none() {
            let buy_amount = params.amount.min(params.minimum_amount);

            // Per-window spend + volume exposure caps (ADR 0012 §2): the
            // order's full notional counts as committed the moment it is
            // placed — even a fully-fooled oracle is bounded.
            let notional = params.price * buy_amount as f64;
            if !exposure.buy_within_caps(notional, buy_amount, params.resource, credits, credit_reserve) {
                return;
            }

            let order_params = CreateOrderParams::new(
                OrderType::Buy,
                market_resource_type,
                params.price,
                buy_amount,
                Some(params.room_name),
            );

            match create_order(&order_params) {
                Ok(()) => {
                    exposure.commit_buy(notional, buy_amount, params.resource);
                    info!(
                        "Placed buy order! Room: {} Resource: {:?} Price: {} Amount: {}",
                        params.room_name, params.resource, params.price, buy_amount
                    );
                }
                Err(err) => {
                    info!(
                        "Failed to place buy order! Error: {:?} Room: {} Resource: {:?} Price: {} Amount: {}",
                        err, params.room_name, params.resource, params.price, buy_amount
                    );
                }
            }
        }
    }

    fn sell_active_orders(
        source_room_name: RoomName,
        terminal: &StructureTerminal,
        order_cache: &mut OrderCache,
        active_orders: &[ActiveSellOrderParameters],
        my_orders: &JsHashMap<String, MyOrder>,
        exposure: &mut ExposureLedger,
    ) -> bool {
        if terminal.cooldown() > 0 {
            return true;
        }

        if terminal.store().get_used_capacity(Some(ResourceType::Energy)) == 0 {
            return false;
        }

        let exposure_view = &*exposure;
        let best_deal = active_orders
            .iter()
            .flat_map(move |params| {
                order_cache.get_orders(MarketResourceType::Resource(params.resource))
                    .iter()
                    .filter(|o| o.order_type() == OrderType::Buy)
                    .filter(|o| o.remaining_amount() > params.minimum_sale_amount && o.price() >= params.minimum_price)
                    .filter(|o| my_orders.get(String::from(o.id())).is_none())
                    .filter_map(|o| {
                        o.room_name().and_then(|order_room_name_js| {
                            let order_room_name: RoomName = order_room_name_js.as_string()?.parse().ok()?;
                            let transfer_amount = o.remaining_amount().min(params.amount);

                            if transfer_amount > 0 {
                                let transfer_cost_per_unit = calc_transaction_cost_fractional(source_room_name, order_room_name);

                                // Hard distance ceiling on deals we initiate
                                // (ADR 0012 §3, threat T5): a far-room
                                // honeypot burns the dealer's energy even at
                                // a nominally good price.
                                if transfer_cost_per_unit > MAX_DEAL_COST_PER_UNIT {
                                    return None;
                                }

                                let energy_transfer_cost_per_unit = transfer_cost_per_unit * params.energy_cost;
                                let effective_price_per_unit = o.price() - energy_transfer_cost_per_unit;

                                if effective_price_per_unit >= params.minimum_price {
                                    let available_transfer_energy = params.maximum_transfer_energy.min(params.available_transfer_energy);
                                    let transferable_units_by_energy =
                                        (available_transfer_energy as f64 / energy_transfer_cost_per_unit) as u32;

                                    let transferable_units = transfer_amount.min(transferable_units_by_energy);

                                    if transferable_units >= params.minimum_sale_amount
                                        && exposure_view.volume_within_cap(params.resource, transferable_units)
                                    {
                                        let transfer_cost = (energy_transfer_cost_per_unit * transferable_units as f64).ceil();

                                        // Tripwire (IBEX-046): the comparator below coalesces
                                        // NaN to Equal; assert finiteness at the source instead.
                                        debug_assert!(transfer_cost.is_finite(), "transfer cost not finite: {transfer_cost}");
                                        debug_assert!(
                                            effective_price_per_unit.is_finite(),
                                            "effective price not finite: {effective_price_per_unit}"
                                        );

                                        // Deal the energy-bounded amount: dealing the un-capped
                                        // transfer_amount made the engine reject the whole deal
                                        // whenever terminal energy < transfer cost (ADR 0012
                                        // M1(c) / IBEX-018).
                                        return Some((o.id(), o.price(), params.resource, transferable_units, transfer_cost, effective_price_per_unit));
                                    }
                                }
                            }

                            None
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .max_by(|a, b| a.4.partial_cmp(&b.4).unwrap_or(std::cmp::Ordering::Equal));

        if let Some((order_id, order_price, resource, transferable_units, transfer_cost, effective_price_per_unit)) = best_deal {
            match deal(&order_id, transferable_units, Some(source_room_name)) {
                Ok(()) => {
                    exposure.commit_volume(resource, transferable_units);
                    info!(
                        "Completed deal! Room: {} Resource: {:?} Amount: {} Transfer Cost: {} Price: {} Effective Price: {} Id: {}",
                        source_room_name,
                        resource,
                        transferable_units,
                        transfer_cost,
                        order_price,
                        effective_price_per_unit,
                        order_id
                    );
                }
                Err(err) => {
                    info!("Failed to complete deal! Error: {:?} Room: {} Resource: {:?} Amount: {} Transfer Cost: {} Price: {} Effective Price: {} Id: {}", err, source_room_name, resource, transferable_units, transfer_cost, order_price, effective_price_per_unit, order_id);
                }
            };

            true
        } else {
            false
        }
    }
}

struct OrderCache {
    orders: HashMap<MarketResourceType, Vec<Order>>,
}

impl OrderCache {
    fn new() -> OrderCache {
        OrderCache { orders: HashMap::new() }
    }

    fn get_orders(&mut self, resource_type: MarketResourceType) -> &Vec<Order> {
        self.orders.entry(resource_type).or_insert_with(|| {
            let filter = LodashFilter::new();
            filter.resource_type(resource_type);
            game::market::get_all_orders(Some(&filter))
        })
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for OrderQueueSystem {
    type SystemData = OrderQueueSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        let features = *data.features;
        let can_buy = features.market.buy && game::market::credits() > features.market.credit_reserve;
        let can_sell = features.market.sell;

        // One-shot market-memory load from the dedicated segment: request
        // until active, read once, then go write-only (the segment leaves
        // the active set, staying clear of the 10-active-segment budget).
        // Trading gates on `loaded` so exposure commitments made before the
        // read could never be overwritten by it.
        if (can_buy || can_sell) && !data.market_memory.loaded {
            data.memory_arbiter.request(MARKET_SEGMENT);
            if data.memory_arbiter.is_active(MARKET_SEGMENT) {
                *data.market_memory = load_market_memory(&data.memory_arbiter);
            }
        }

        let can_run = game::time().is_multiple_of(20)
            && data.governor.can_execute_cpu(CpuBar::HighPriority)
            && (can_buy || can_sell)
            && data.market_memory.loaded;

        if can_run {
            let mut order_cache = OrderCache::new();

            let my_orders = game::market::orders();

            let complete_orders = my_orders.values().filter(|order| order.remaining_amount() == 0);

            for order in complete_orders {
                let _ = game::market::cancel_order(&order.id());
            }

            if !data.order_queue.rooms.is_empty() {
                data.market_memory.exposure.roll(game::time());

                // Pricing references the FairValue oracle's volume-filtered
                // trailing median — never a raw latest history day, which is
                // exactly what a wash-trader paints (ADR 0012 §1 / IBEX-018).
                // `None` = untrusted history (thin, short, or absent): abstain.
                let mut fair_values: HashMap<MarketResourceType, Option<FairValue>> = HashMap::new();

                let credits = game::market::credits();
                let credit_reserve = features.market.credit_reserve;

                // Anomalous-day skips aggregate into ONE warn per pass — a
                // painted market day lasts a real-time day, and a per-site
                // warn would repeat across rooms x resources every pass.
                let mut anomalous_resources: std::collections::HashSet<ResourceType> = std::collections::HashSet::new();

                for (room_name, room_data) in &data.order_queue.rooms {
                    if let Some(terminal) = game::rooms().get(*room_name).and_then(|r| r.terminal()) {
                        if can_sell {
                            for entry in &room_data.outgoing_passive_requests {
                                if let Some(fair) = cached_fair_value(&mut fair_values, &mut data.market_memory, entry.resource) {
                                    // Maker sells stay safe on an anomalous day —
                                    // the ask is anchored to the median, not the
                                    // painted day.
                                    Self::sell_passive_order(
                                        &my_orders,
                                        &mut data.market_memory.exposure,
                                        PassiveOrderParameters {
                                            room_name: *room_name,
                                            resource: entry.resource,
                                            amount: entry.amount,
                                            minimum_amount: TRADE_BLOCK_UNITS,
                                            price: fair.price * MAKER_SELL_MARKUP,
                                        },
                                    );
                                }
                            }

                            let active_orders: Vec<_> = room_data
                                .outgoing_active_requests
                                .iter()
                                .filter_map(|entry| {
                                    let fair = cached_fair_value(&mut fair_values, &mut data.market_memory, entry.resource)?;
                                    let energy_fair = cached_fair_value(&mut fair_values, &mut data.market_memory, ResourceType::Energy)?;

                                    // An anomalous latest day bars taker action for
                                    // the cadence (ADR 0012 §1): dealing into a
                                    // painted/spiked book is the T1/T2 payoff path.
                                    if fair.latest_day_anomalous || energy_fair.latest_day_anomalous {
                                        debug!(
                                            "Anomalous market history day - skipping active sales. Room: {} Resource: {:?}",
                                            room_name, entry.resource
                                        );
                                        anomalous_resources.insert(if fair.latest_day_anomalous {
                                            entry.resource
                                        } else {
                                            ResourceType::Energy
                                        });
                                        return None;
                                    }

                                    Some(ActiveSellOrderParameters {
                                        resource: entry.resource,
                                        amount: entry.amount,
                                        minimum_sale_amount: TRADE_BLOCK_UNITS,
                                        minimum_price: fair.price * ACTIVE_SELL_FLOOR_FRACTION,
                                        available_transfer_energy: entry.available_transfer_energy,
                                        maximum_transfer_energy: OrderQueue::maximum_transfer_energy(),
                                        energy_cost: energy_fair.price,
                                    })
                                })
                                .collect();

                            let _terminal_busy = Self::sell_active_orders(
                                *room_name,
                                &terminal,
                                &mut order_cache,
                                &active_orders,
                                &my_orders,
                                &mut data.market_memory.exposure,
                            );
                        }

                        if can_buy {
                            for entry in &room_data.incoming_passive_requests {
                                if let Some(fair) = cached_fair_value(&mut fair_values, &mut data.market_memory, entry.resource) {
                                    // Painted-day rule (ADR 0012 M1): an anomalous
                                    // latest day produces zero buy actions.
                                    if fair.latest_day_anomalous {
                                        debug!(
                                            "Anomalous market history day - skipping buys. Room: {} Resource: {:?}",
                                            room_name, entry.resource
                                        );
                                        anomalous_resources.insert(entry.resource);
                                        continue;
                                    }

                                    Self::buy_passive_order(
                                        &my_orders,
                                        &mut data.market_memory.exposure,
                                        credits,
                                        credit_reserve,
                                        PassiveOrderParameters {
                                            room_name: *room_name,
                                            resource: entry.resource,
                                            amount: entry.amount,
                                            minimum_amount: TRADE_BLOCK_UNITS,
                                            // Maker buys bid BELOW fair value — the old
                                            // `avg + 0.1σ` bid above it, donating the
                                            // spread (ADR 0012 §2 / IBEX-018).
                                            price: fair.price * MAKER_BUY_DISCOUNT,
                                        },
                                    );
                                }
                            }
                        }
                    }
                }

                if !anomalous_resources.is_empty() {
                    // Genuine attack signal (deviation/z gate hits), one
                    // aggregated line per pass; per-room detail is at debug.
                    warn!(
                        "Anomalous market history day - taker/buy actions skipped this pass for: {:?}",
                        anomalous_resources
                    );
                }

                // Persist the pass's history merges + exposure commitments.
                save_market_memory(&mut data.memory_arbiter, &data.market_memory);
            }
        }

        data.order_queue.clear();
    }
}
