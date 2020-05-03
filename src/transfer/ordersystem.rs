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
}

impl OrderQueueRoomData {
    pub fn new() -> OrderQueueRoomData {
        OrderQueueRoomData {
            outgoing_passive_requests: Vec::new(),
            outgoing_active_requests: Vec::new(),
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

    pub fn clear(&mut self) {
        self.rooms.clear();
    }

    fn visualize(&self, _ui: &mut UISystem, _visualizer: &mut Visualizer) {
        if crate::features::transfer::visualize_orders() {
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
    visualizer: Option<Write<'a, Visualizer>>,
    ui: Option<Write<'a, UISystem>>,
}

struct PassiveSellOrderParameters {
    room_name: RoomName,
    resource: ResourceType,
    amount: u32,
    minimum_sale_amount: u32,
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
    fn sell_passive_order(my_orders: &HashMap<String, MyOrder>, params: PassiveSellOrderParameters) {
        if params.amount < params.minimum_sale_amount {
            //TODO: Handle order in progress, cancel etc.?
            return;
        }

        let market_resource_type = MarketResourceType::Resource(params.resource);

        let current_orders: Vec<_> = my_orders
            .values()
            .filter(|o| o.order_type == OrderType::Sell && o.resource_type == market_resource_type)
            .filter(|o| o.remaining_amount > 0)
            .filter(|o| {
                o.room_name
                    .map(|order_room_name| order_room_name == params.room_name)
                    .unwrap_or(false)
            })
            .collect();

        //
        // NOTE: Sell in block of minimum sale amount, not total capacity.
        //

        if current_orders.is_empty() {
            let sell_amount = params.minimum_sale_amount;

            match create_order(
                OrderType::Sell,
                market_resource_type,
                params.price,
                sell_amount,
                Some(params.room_name),
            ) {
                ReturnCode::Ok => {
                    info!(
                        "Placed sell order! Room: {} Resource: {:?} Price: {} Amount: {}",
                        params.room_name, params.resource, params.price, sell_amount
                    );
                }
                err => {
                    info!(
                        "Failed to place sell order! Error: {:?} Room: {} Resource: {:?} Price: {} Amount: {}",
                        err, params.room_name, params.resource, params.price, sell_amount
                    );
                }
            }
        }
    }

    fn calc_transaction_cost_fractional(from: RoomName, to: RoomName) -> f64 {
        let distance = game::map::get_room_linear_distance(from, to, true) as f64;

        1.0 - (-distance / 30.0).exp()
    }

    fn sell_active_orders(
        source_room_name: RoomName,
        terminal: &StructureTerminal,
        order_cache: &mut OrderCache,
        active_orders: &[ActiveSellOrderParameters],
    )
     {
        if terminal.cooldown() > 0 || terminal.store_used_capacity(Some(ResourceType::Energy)) == 0 {
            return;
        }

        active_orders
            .iter()
            .flat_map(move |params| {
                order_cache.get_orders(MarketResourceType::Resource(params.resource))
                    .iter()
                    .filter(|o| o.order_type == OrderType::Buy)
                    .filter(|o| o.remaining_amount > params.minimum_sale_amount && o.price >= params.minimum_price)
                    .filter_map(|o| {
                        o.room_name.and_then(|order_room_name| {
                            let transfer_amount = o.remaining_amount.min(params.amount);

                            if transfer_amount > 0 {
                                let transfer_cost_per_unit = Self::calc_transaction_cost_fractional(source_room_name, order_room_name);
                                let energy_transfer_cost_per_unit = transfer_cost_per_unit * params.energy_cost;
                                let effective_price_per_unit = o.price - energy_transfer_cost_per_unit;

                                if effective_price_per_unit >= params.minimum_price {
                                    let available_transfer_energy = params.maximum_transfer_energy.min(params.available_transfer_energy);
                                    let transferable_units_by_energy =
                                        (available_transfer_energy as f64 / energy_transfer_cost_per_unit) as u32;

                                    let transferable_units = transfer_amount.min(transferable_units_by_energy);

                                    if transferable_units >= params.minimum_sale_amount {
                                        let transfer_cost = (energy_transfer_cost_per_unit * transferable_units as f64).ceil();

                                        return Some((o.id.to_owned(), o.price, params.resource, transfer_amount, transfer_cost, effective_price_per_unit));
                                    }
                                }
                            }

                            None
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .max_by(|a, b| a.4.partial_cmp(&b.4).unwrap())
            .map(|(order_id, order_price, resource, transfer_amount, transfer_cost, effective_price_per_unit)| {
                match deal(&order_id, transfer_amount, Some(source_room_name)) {
                    ReturnCode::Ok => {
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
                    err => {
                        info!("Failed to complete deal! Error: {:?} Room: {}Resource: {:?} Amount: {} Transfer Cost: {} Price: {} Effectice Price: {} Id: {}", err, source_room_name, resource, transfer_amount, transfer_cost, order_price, effective_price_per_unit, order_id);
                    }
                }
            });
    }
}

struct OrderCache {
    orders: HashMap<MarketResourceType, Vec<Order>>
}

impl OrderCache {
    fn new() -> OrderCache {
        OrderCache {
            orders: HashMap::new()
        }
    }

    fn get_orders(&mut self, resource_type: MarketResourceType) -> &Vec<Order> {
        self.orders.entry(resource_type)
            .or_insert_with(|| game::market::get_all_orders(Some(resource_type)))
    }
}

#[cfg_attr(feature = "profile", screeps_timing_annotate::timing)]
impl<'a> System<'a> for OrderQueueSystem {
    type SystemData = OrderQueueSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        if let Some(visualizer) = &mut data.visualizer {
            if let Some(ui) = &mut data.ui {
                data.order_queue.visualize(ui, visualizer);
            }
        }

        let can_buy = crate::features::market::buy();
        let can_sell = crate::features::market::sell();

        if !can_buy && !can_sell {
            return;
        }

        if game::time() % 50 != 0 || game::cpu::bucket() <= game::cpu::tick_limit() {
            return;
        }

        let mut order_cache = OrderCache::new();

        let my_orders = game::market::orders();

        let complete_orders = my_orders.values().filter(|order| order.remaining_amount == 0);

        for order in complete_orders {
            game::market::cancel_order(&order.id);
        }

        if !data.order_queue.rooms.is_empty() {
            let mut resource_history = HashMap::new();

            for (room_name, room_data) in &data.order_queue.rooms {
                if let Some(terminal) = game::rooms::get(*room_name).and_then(|r| r.terminal()) {
                    if can_sell {
                        for entry in &room_data.outgoing_passive_requests {
                            //
                            // NOTE: This current relies on the orders being in sequential date order.
                            //

                            let market_resource = MarketResourceType::Resource(entry.resource);

                            let _ = resource_history
                                .entry(market_resource)
                                .or_insert_with(|| game::market::get_history(Some(market_resource)));

                            //TODO: Validate that the current average price is sane (compare to prior day?).
                            //TODO: Need better pricing calculations.

                            if let Some(latest_resource_history) = resource_history.get(&market_resource).unwrap().last() {
                                Self::sell_passive_order(
                                    &my_orders,
                                    PassiveSellOrderParameters {
                                        room_name: *room_name,
                                        resource: entry.resource,
                                        amount: entry.amount,
                                        minimum_sale_amount: 2000,
                                        price: latest_resource_history.avg_price + (latest_resource_history.stddev_price * 0.1),
                                    },
                                );
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
                                    .or_insert_with(|| game::market::get_history(Some(market_resource)));

                                let energy_market_resource = MarketResourceType::Resource(ResourceType::Energy);

                                let _ = resource_history
                                    .entry(energy_market_resource)
                                    .or_insert_with(|| game::market::get_history(Some(energy_market_resource)));

                                if let Some(latest_resource_history) = resource_history.get(&market_resource).unwrap().last() {
                                    if let Some(latest_energy_history) = resource_history.get(&energy_market_resource).unwrap().last() {
                                        return Some(ActiveSellOrderParameters {
                                            resource: entry.resource,
                                            amount: entry.amount,
                                            minimum_sale_amount: 2000,
                                            minimum_price: latest_resource_history.avg_price - (latest_resource_history.stddev_price * 0.2),
                                            available_transfer_energy: entry.available_transfer_energy,
                                            maximum_transfer_energy: OrderQueue::maximum_transfer_energy(),
                                            energy_cost: latest_energy_history.avg_price - (latest_energy_history.stddev_price * 0.2),
                                        });
                                    }
                                }

                                None
                            })
                            .collect();                       

                        Self::sell_active_orders(*room_name, &terminal, &mut order_cache, &active_orders);
                    }
                }
            }
        }

        data.order_queue.clear();
    }
}
