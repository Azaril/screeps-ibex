use crate::ui::*;
use crate::visualize::*;
use screeps::*;
use screeps::game::market::*;
use specs::prelude::{Entities, LazyUpdate, Read, ResourceId, System, SystemData, World, Write, WriteStorage};
use std::collections::HashMap;
use crate::room::data::*;
use log::*;

pub struct OrderQueueResourceData {
    passive_sale: u32,
    active_sale: u32,
}

impl OrderQueueResourceData {
    pub fn new() -> OrderQueueResourceData {
        OrderQueueResourceData {
            passive_sale: 0,
            active_sale: 0,
        }
    }
}

pub struct OrderQueueRoomData {
    outgoing_requests: HashMap<ResourceType, OrderQueueResourceData>,
}

impl OrderQueueRoomData {
    pub fn new() -> OrderQueueRoomData {
        OrderQueueRoomData {
            outgoing_requests: HashMap::new(),
        }
    }
}

#[derive(Default)]
pub struct OrderQueue {
    rooms: HashMap<RoomName, OrderQueueRoomData>,
}

impl OrderQueue {
    pub fn maximum_transfer_cost(&self) -> u32 {
        5000
    }

    pub fn get_room(&mut self, room: RoomName) -> &mut OrderQueueRoomData {
        self.rooms.entry(room).or_insert_with(OrderQueueRoomData::new)
    }

    pub fn request_passive_sale(&mut self, room: RoomName, resource: ResourceType, amount: u32) {
        let resource_entry = self.get_room(room)
            .outgoing_requests
            .entry(resource)
            .or_insert_with(OrderQueueResourceData::new);

        resource_entry.passive_sale += amount;
    }

    pub fn request_active_sale(&mut self, room: RoomName, resource: ResourceType, amount: u32) {
        let resource_entry = self.get_room(room)
            .outgoing_requests
            .entry(resource)
            .or_insert_with(OrderQueueResourceData::new);

        resource_entry.active_sale += amount;
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

struct ActiveSellOrderParameters<'a> {
    room_name: RoomName,
    terminal: &'a StructureTerminal,
    resource: ResourceType,
    amount: u32, 
    minimum_sale_amount: u32, 
    minimum_price: f64, 
    maximum_transfer_cost: f64,
    energy_cost: f64
}

pub struct OrderQueueSystem;

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
            .filter(|o| o.room_name.map(|order_room_name| order_room_name == params.room_name).unwrap_or(false))
            .collect();

        //
        // NOTE: Sell in block of minimum sale amount, not total capacity.
        //

        if current_orders.is_empty() {
            let sell_amount = params.minimum_sale_amount;

            match create_order(OrderType::Sell, market_resource_type, params.price, sell_amount, Some(params.room_name)) {
                ReturnCode::Ok => {
                    info!("Placed sell order! Room: {} Resource: {:?} Price: {} Amount: {}", params.room_name, params.resource, params.price, sell_amount);
                },
                err => {
                    info!("Failed to place sell order! Error: {:?} Room: {} Resource: {:?} Price: {} Amount: {}", err, params.room_name, params.resource, params.price, sell_amount);
                }
            }
        }
    }
    
    fn sell_active_order(orders: &[Order], params: ActiveSellOrderParameters) {
        if params.amount == 0 || params.terminal.cooldown() > 0 || params.terminal.store_used_capacity(Some(ResourceType::Energy)) == 0 {
            return;
        }

        let best_order = orders
            .iter()
            .filter(|o| o.order_type == OrderType::Buy && o.resource_type == MarketResourceType::Resource(params.resource))
            .filter(|o| o.remaining_amount > params.minimum_sale_amount && o.price >= params.minimum_price)
            .filter_map(|o| {
                o.room_name.and_then(|order_room_name| { 
                    let transfer_amount = o.remaining_amount.min(params.amount);

                    if transfer_amount > 0 {
                        let transfer_cost = calc_transaction_cost(transfer_amount, params.room_name, order_room_name);
                        let transfer_cost_per_unit = transfer_cost / (transfer_amount as f64);
                        let energy_transfer_cost_per_unit = transfer_cost_per_unit * params.energy_cost;

                        let effective_price_per_unit = o.price - energy_transfer_cost_per_unit;

                        if effective_price_per_unit >= params.minimum_price && transfer_cost <= params.maximum_transfer_cost {
                            return Some((o, transfer_amount, transfer_cost, effective_price_per_unit))
                        }
                    }

                    None
                })
            })
            .max_by(|a, b| a.3.partial_cmp(&b.3).unwrap());

        if let Some((best_order, transfer_amount, transfer_cost, effective_price_per_unit)) = best_order {
            let terminal_energy = params.terminal.store_used_capacity(Some(ResourceType::Energy));
            
            if transfer_cost <= terminal_energy as f64 {
                match deal(&best_order.id, transfer_amount, Some(params.room_name)) {
                    ReturnCode::Ok => {
                        info!("Completed deal! Room: {} Resource: {:?} Amount: {} Transfer Cost: {} Price: {} Effective Price: {} Id: {}", params.room_name, params.resource, transfer_amount, transfer_cost, best_order.price, effective_price_per_unit, best_order.id);
                    },
                    err => {
                        info!("Failed to complete deal! Error: {:?} Room: {}Resource: {:?} Amount: {} Transfer Cost: {} Price: {} Effectice Price: {} Id: {}", err, params.room_name, params.resource, transfer_amount, transfer_cost, best_order.price, effective_price_per_unit, best_order.id);
                
                    }
                }
            }
        }
    }
}

impl<'a> System<'a> for OrderQueueSystem {
    type SystemData = OrderQueueSystemData<'a>;

    fn run(&mut self, mut data: Self::SystemData) {
        if let Some(visualizer) = &mut data.visualizer {
            if let Some(ui) = &mut data.ui {
                data.order_queue.visualize(ui, visualizer);
            }
        }

        if game::time() % 50 != 0 {
            return;
        }

        let orders = game::market::get_all_orders();
        let my_orders = game::market::orders();

        let complete_orders = my_orders
            .values()
            .filter(|order| order.remaining_amount == 0);

        for order in complete_orders {
            game::market::cancel_order(&order.id);
        }

        if !data.order_queue.rooms.is_empty() {         
            let mut resource_history = HashMap::new();

            for (room_name, room_data) in &data.order_queue.rooms {
                if let Some(terminal) = game::rooms::get(*room_name).and_then(|r| r.terminal()) {                   
                    for (resource, entry) in &room_data.outgoing_requests {
                        //
                        // NOTE: This current relies on the orders being in sequential date order.
                        //

                        let market_resource = MarketResourceType::Resource(*resource);

                        let _ = resource_history
                            .entry(market_resource)
                            .or_insert_with(|| game::market::get_history(Some(market_resource)));

                        let energy_market_resource = MarketResourceType::Resource(*resource);

                        let _ = resource_history
                            .entry(energy_market_resource)
                            .or_insert_with(|| game::market::get_history(Some(energy_market_resource)));
                        
                        //TODO: Validate that the current average price is sane (compare to prior day?).
                        //TODO: Need better pricing calculations.

                        if let Some(latest_history) = resource_history.get(&market_resource).unwrap().last() {
                            if let Some(latest_energy_history) = resource_history.get(&energy_market_resource).unwrap().last() {
                                if entry.passive_sale > 0 {
                                    Self::sell_passive_order(&my_orders, PassiveSellOrderParameters {
                                        room_name: *room_name,
                                        resource: *resource,
                                        amount: entry.passive_sale,
                                        minimum_sale_amount: 2000,
                                        price: latest_history.avg_price + (latest_history.stddev_price * 0.1),
                                    });
                                }

                                if entry.active_sale > 0 {
                                    Self::sell_active_order(&orders, ActiveSellOrderParameters {
                                        room_name: *room_name,
                                        terminal: &terminal,
                                        resource: *resource,
                                        amount: entry.active_sale, 
                                        minimum_sale_amount: 2000, 
                                        minimum_price: latest_history.avg_price - (latest_history.stddev_price * 0.2), 
                                        maximum_transfer_cost: data.order_queue.maximum_transfer_cost() as f64,
                                        energy_cost: latest_energy_history.avg_price
                                    });
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
