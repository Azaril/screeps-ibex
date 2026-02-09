use super::utility::*;
use crate::missions::constants::*;
use crate::room::data::*;
use crate::ui::*;
use crate::visualize::*;
use log::*;
use screeps::game::market::*;
use screeps::*;
use specs::prelude::{Entities, LazyUpdate, Read, ResourceId, System, SystemData, World, Write, WriteStorage};
use std::collections::HashMap;

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
    rooms: HashMap<RoomName, OrderQueueRoomData>,
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

    fn visualize(&self, _ui: &mut UISystem, _visualizer: &mut Visualizer) {
        if crate::features::features().transfer.visualize.orders() {
            /*
            for (room_name, room) in &self.rooms {
                ui.with_room(*room_name, visualizer, |_room_ui| {
                    //TODO: Visualize orders.
                });
            }
            */
        }
    }
}

#[derive(SystemData)]
pub struct OrderQueueSystemData<'a> {
    order_queue: Write<'a, OrderQueue>,
    updater: Read<'a, LazyUpdate>,
    entities: Entities<'a>,
    room_data: WriteStorage<'a, RoomData>,
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
    fn sell_passive_order(my_orders: &JsHashMap<String, MyOrder>, params: PassiveOrderParameters) {
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

            let order_params = CreateOrderParams::new(
                OrderType::Sell,
                market_resource_type,
                params.price,
                sell_amount,
                Some(params.room_name),
            );

            match create_order(&order_params) {
                Ok(()) => {
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

    fn buy_passive_order(my_orders: &JsHashMap<String, MyOrder>, params: PassiveOrderParameters) {
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

            let order_params = CreateOrderParams::new(
                OrderType::Buy,
                market_resource_type,
                params.price,
                buy_amount,
                Some(params.room_name),
            );

            match create_order(&order_params) {
                Ok(()) => {
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
    ) -> bool {
        if terminal.cooldown() > 0 {
            return true;
        }

        if terminal.store().get_used_capacity(Some(ResourceType::Energy)) == 0 {
            return false;
        }

        active_orders
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
                                let energy_transfer_cost_per_unit = transfer_cost_per_unit * params.energy_cost;
                                let effective_price_per_unit = o.price() - energy_transfer_cost_per_unit;

                                if effective_price_per_unit >= params.minimum_price {
                                    let available_transfer_energy = params.maximum_transfer_energy.min(params.available_transfer_energy);
                                    let transferable_units_by_energy =
                                        (available_transfer_energy as f64 / energy_transfer_cost_per_unit) as u32;

                                    let transferable_units = transfer_amount.min(transferable_units_by_energy);

                                    if transferable_units >= params.minimum_sale_amount {
                                        let transfer_cost = (energy_transfer_cost_per_unit * transferable_units as f64).ceil();

                                        return Some((o.id(), o.price(), params.resource, transfer_amount, transfer_cost, effective_price_per_unit));
                                    }
                                }
                            }

                            None
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .max_by(|a, b| a.4.partial_cmp(&b.4).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(order_id, order_price, resource, transfer_amount, transfer_cost, effective_price_per_unit)| {
                match deal(&order_id, transfer_amount, Some(source_room_name)) {
                    Ok(()) => {
                        info!(
                            "Completed deal! Room: {} Resource: {:?} Amount: {} Transfer Cost: {} Price: {} Effective Price: {} Id: {}",
                            source_room_name,
                            resource,
                            transfer_amount,
                            transfer_cost,
                            order_price,
                            effective_price_per_unit,
                            order_id
                        );
                    }
                    Err(err) => {
                        info!("Failed to complete deal! Error: {:?} Room: {}Resource: {:?} Amount: {} Transfer Cost: {} Price: {} Effectice Price: {} Id: {}", err, source_room_name, resource, transfer_amount, transfer_cost, order_price, effective_price_per_unit, order_id);
                    }
                };

                true
            })
            .unwrap_or(false)
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
        let features = crate::features::features();
        let can_buy = features.market.buy && game::market::credits() > features.market.credit_reserve;
        let can_sell = features.market.sell;

        let can_run = game::time().is_multiple_of(20) && can_execute_cpu(CpuBar::HighPriority) && (can_buy || can_sell);

        if can_run {
            let mut order_cache = OrderCache::new();

            let my_orders = game::market::orders();

            let complete_orders = my_orders.values().filter(|order| order.remaining_amount() == 0);

            for order in complete_orders {
                let _ = game::market::cancel_order(&order.id());
            }

            if !data.order_queue.rooms.is_empty() {
                let mut resource_history = HashMap::new();

                let can_trust_history = |history: &OrderHistoryRecord| {
                    history.transactions() > 100 && history.volume() > 1000 && (history.stddev_price() <= history.avg_price() * 0.5)
                };

                for (room_name, room_data) in &data.order_queue.rooms {
                    if let Some(terminal) = game::rooms().get(*room_name).and_then(|r| r.terminal()) {
                        if can_sell {
                            for entry in &room_data.outgoing_passive_requests {
                                //
                                // NOTE: This current relies on the orders being in sequential date order.
                                //

                                let market_resource = MarketResourceType::Resource(entry.resource);

                                let _ = resource_history
                                    .entry(market_resource)
                                    .or_insert_with(|| game::market::get_history(Some(entry.resource)));

                                //TODO: Validate that the current average price is sane (compare to prior day?).
                                //TODO: Need better pricing calculations.

                                if let Some(latest_resource_history) = resource_history.get(&market_resource).and_then(|v| v.last()) {
                                    if can_trust_history(latest_resource_history) {
                                        Self::sell_passive_order(
                                            &my_orders,
                                            PassiveOrderParameters {
                                                room_name: *room_name,
                                                resource: entry.resource,
                                                amount: entry.amount,
                                                minimum_amount: 2000,
                                                price: latest_resource_history.avg_price() + (latest_resource_history.stddev_price() * 0.1),
                                            },
                                        );
                                    }
                                }
                            }

                            let active_orders: Vec<_> = room_data
                                .outgoing_active_requests
                                .iter()
                                .filter_map(|entry| {
                                    //
                                    // NOTE: This current relies on the orders being in sequential date order.
                                    //

                                    let market_resource = MarketResourceType::Resource(entry.resource);

                                    let _ = resource_history
                                        .entry(market_resource)
                                        .or_insert_with(|| game::market::get_history(Some(entry.resource)));

                                    let energy_market_resource = MarketResourceType::Resource(ResourceType::Energy);

                                    let _ = resource_history
                                        .entry(energy_market_resource)
                                        .or_insert_with(|| game::market::get_history(Some(ResourceType::Energy)));

                                    if let Some(latest_resource_history) = resource_history.get(&market_resource).and_then(|v| v.last()) {
                                        if let Some(latest_energy_history) =
                                            resource_history.get(&energy_market_resource).and_then(|v| v.last())
                                        {
                                            if can_trust_history(latest_resource_history) && can_trust_history(latest_energy_history) {
                                                return Some(ActiveSellOrderParameters {
                                                    resource: entry.resource,
                                                    amount: entry.amount,
                                                    minimum_sale_amount: 2000,
                                                    minimum_price: latest_resource_history.avg_price()
                                                        - (latest_resource_history.stddev_price() * 0.2),
                                                    available_transfer_energy: entry.available_transfer_energy,
                                                    maximum_transfer_energy: OrderQueue::maximum_transfer_energy(),
                                                    energy_cost: latest_energy_history.avg_price()
                                                        - (latest_energy_history.stddev_price() * 0.2),
                                                });
                                            }
                                        }
                                    }

                                    None
                                })
                                .collect();

                            let _terminal_busy =
                                Self::sell_active_orders(*room_name, &terminal, &mut order_cache, &active_orders, &my_orders);
                        }

                        if can_buy {
                            for entry in &room_data.incoming_passive_requests {
                                //
                                // NOTE: This current relies on the orders being in sequential date order.
                                //

                                let market_resource = MarketResourceType::Resource(entry.resource);

                                let _ = resource_history
                                    .entry(market_resource)
                                    .or_insert_with(|| game::market::get_history(Some(entry.resource)));

                                //TODO: Validate that the current average price is sane (compare to prior day?).
                                //TODO: Need better pricing calculations.

                                if let Some(latest_resource_history) = resource_history.get(&market_resource).and_then(|v| v.last()) {
                                    if can_trust_history(latest_resource_history) {
                                        Self::buy_passive_order(
                                            &my_orders,
                                            PassiveOrderParameters {
                                                room_name: *room_name,
                                                resource: entry.resource,
                                                amount: entry.amount,
                                                minimum_amount: 2000,
                                                price: latest_resource_history.avg_price() + (latest_resource_history.stddev_price() * 0.1),
                                            },
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        data.order_queue.clear();
    }
}
