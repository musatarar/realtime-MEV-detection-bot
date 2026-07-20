use std::{collections::HashMap};

use alloy_primitives::{Address, U256};

pub type NodeId = usize;

#[derive(Debug, Clone)]
pub struct Node<V> {
    pub data: V,
}

#[derive(Debug, Clone)]
pub struct Edge<E> {
    pub target: NodeId,
    pub data: E
}

#[derive(Debug, Clone)]
pub struct Graph<V, E> {
    nodes: Vec<Node<V>>,
    adj_list: Vec<Vec<Edge<E>>>,
}

impl<V, E> Graph<V, E> {
    /// Creates new empty graph
    pub fn new() -> Self {
        Self { nodes: Vec::new(), adj_list: Vec::new() }
    }

    // preallocate capacity for `n` nodes
    pub fn with_capacity(n: usize) -> Self {
        Self { nodes: Vec::with_capacity(n), adj_list: Vec::with_capacity(n) }
    }

    /// Add a node and return its ID
    pub fn add_node(&mut self, data: V) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node {data: data});
        self.adj_list.push(Vec::new());
        id
    }

    /// Add a directed edge from source to target
    pub fn add_edge(&mut self, source: NodeId, target: NodeId, data: E) {
        assert !(
            source < self.nodes.len() && target < self.nodes.len(),
            "Nodes must exist"
        );
        self.adj_list[source].push(Edge { target: target, data: data });
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.adj_list.iter().map(Vec::len).sum()
    }

    pub fn get_node(&self, node_id: NodeId) -> Option<&V> {
        self.nodes.get(node_id).map(|n| &n.data)
    }

    pub fn get_node_mut(&mut self, node_id: NodeId) -> Option<&mut V> {
        self.nodes.get_mut(node_id).map(|n| &mut n.data)
    }

    /// Neighbors of a specific node
    pub fn get_neighbors(&self, node_id: NodeId) -> Option<&[Edge<E>]> {
        self.adj_list.get(node_id).map(Vec::as_slice)
    }

    pub fn get_neighbors_mut(&mut self, node_id: NodeId) -> Option<&mut [Edge<E>]> {
        self.adj_list.get_mut(node_id).map(Vec::as_mut_slice)
    }

    /// Return edges as (source, edge) for Bellman-Ford Algorithm
    pub fn edges(&self) -> impl Iterator<Item = (NodeId, &Edge<E>)> + '_ {
        self.adj_list
            .iter()
            .enumerate()
            .flat_map(|(src, es)| es.iter().map(move |e| (src, e)))
    }

    pub fn edges_mut(&mut self) -> impl Iterator<Item = (NodeId, &mut Edge<E>)> + '_ {
        self.adj_list
            .iter_mut()
            .enumerate()
            .flat_map(|(src, es)| es.iter_mut().map(move |e| (src, e)))
    }

    pub fn edge_at(&self, source: NodeId, idx: usize) -> Option<&Edge<E>>{
        self.adj_list.get(source)?.get(idx)
    }

    pub fn edge_at_mut(&mut self, source: NodeId, idx: usize) -> Option<&mut Edge<E>>{
        self.adj_list.get_mut(source)?.get_mut(idx)
    }

    /// Drop all edges to reset graph when reserves change
    pub fn drop_edges(&mut self) {
        for es in &mut self.adj_list{
            es.clear();
        }
    }
}

impl<V, E> Default for Graph<V, E> {
    fn default() -> Self {
        Self::new()
    }
}


// DEX GRAPH

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetNode {
    pub token: Address,
    pub decimals: u8,
    pub symbol: String
}

impl AssetNode {
    pub fn new(token: Address, decimals: u8, symbol: impl Into<String>) -> Self {
        Self {
            token,
            decimals,
            symbol: symbol.into()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Venue {
    UniV2,
    UniV3,
    Curve
}

#[derive(Debug, Clone)]
pub struct MarketEdge {
    pub pool: Address,
    pub venue: Venue,
    pub token_in: Address,
    pub token_out: Address,
    pub reserve_in: U256,
    pub reserve_out: U256,
    pub decimals_in: u8,
    pub decimals_out: u8,
    pub fee_bps: u32,
    /// `-ln(spot price)`; `f64::INFINITY` when the market cannot quote
    pub weight: f64
}

impl MarketEdge {
    /// Weight is left uninitialised (`INFINITY`) — call `refresh_weight`, or
    /// go through [`MarketGraph::add_market`], which does it for you.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pool: Address,
        venue: Venue,
        token_in: Address,
        token_out: Address,
        reserve_in: U256,
        reserve_out: U256,
        decimals_in: u8,
        decimals_out: u8,
        fee_bps: u32,
    ) -> Self {
        let mut e = Self {
            pool,
            venue,
            token_in,
            token_out,
            reserve_in,
            reserve_out,
            decimals_in,
            decimals_out,
            fee_bps,
            weight: f64::INFINITY,
        };
        e.refresh_weight();
        e
    }

    /// Marginal price of `token_out` per `token_in`, decimal-normalised and
    /// fee-inclusive. Returns `0.0` if the pool can't quote.
    pub fn spot_price(&self) -> f64 {
        let r_in = u256_to_f64(self.reserve_in) / 10f64.powi(self.decimals_in as i32);
        let r_out = u256_to_f64(self.reserve_out) / 10f64.powi(self.decimals_out as i32);

        if r_in <= 0.0 || r_out <= 0.0 || r_in.is_infinite() || r_out.is_infinite() {
            return 0.0;
        }
        (r_out / r_in) * (1.0 - self.fee_bps as f64 / 10_000.0)
    }

    /// Recomputes the cached weight from current reserves.
    ///
    /// A zero/unquotable price maps to `+inf`, matching Zhou, et al's convention
    /// for edges that consume an asset without returning one.
    pub fn refresh_weight(&mut self) {
        let p = self.spot_price();
        self.weight = if p > 0.0 { -p.ln() } else { f64::INFINITY }
    }

    pub fn is_quotable(&self) -> bool {
        self.weight.is_finite()
    }

    /// Constant-product output, exact integer math (UniV2 `getAmountOut`).
    /// `None` on overflow or an empty pool.
    pub fn amount_out(&self, amount_in: U256) -> Option<U256> {
        if amount_in.is_zero() {
            return Some(U256::ZERO);
        }
        if self.reserve_in.is_zero() || self.reserve_out.is_zero() {
            return None;
        }
        if self.venue != Venue::UniV2 {
            return None;
        }
        let fee_num = U256::from(10000u32.checked_sub(self.fee_bps)?);
        let fee_den = U256::from(10000u32);

        let in_with_fee = amount_in.checked_mul(fee_num)?;
        let numerator = in_with_fee.checked_mul(self.reserve_out)?;
        let denominator = self.reserve_in.checked_mul(fee_den)?.checked_add(in_with_fee)?;

        if denominator.is_zero(){
            return None;
        }
        Some(numerator / denominator)
    }

    /// Simulates swap, moving reserves and updating weight; returns output. 
    /// Leaves edge untouched on fail.
    pub fn apply_swap(&mut self, amount_in: U256) -> Option<U256> {
        let out = self.amount_out(amount_in)?;
        if out >= self.reserve_out {
            return None;
        }
        let new_in = self.reserve_in.checked_add(amount_in)?;
        let new_out = self.reserve_out.checked_sub(out)?;
        self.reserve_in = new_in;
        self.reserve_out = new_out;
        self.refresh_weight();
        Some(out)
    }


}

fn u256_to_f64(v: U256) -> f64 {
    let mut out = 0.0f64;
    for (i, limb) in v.as_limbs().iter().enumerate() {
        out += (*limb as f64) * 2f64.powi(64 * i as i32)
    }
    out
}

pub type DexGraph = Graph<AssetNode, MarketEdge>;


/// Wraps [`DexGraph`] with an `Address -> NodeId` index, so you can ingest
/// `TxCategory::UniV2Swap { path, .. }` (which speaks addresses) directly.
#[derive(Debug, Clone)]
pub struct MarketGraph {
    pub graph: DexGraph,
    index: HashMap<Address, NodeId>,
    base: Option<NodeId>,
}

const UNI_V2_FEE_BPS: u32 = 30;

impl MarketGraph {
    pub fn new() -> Self {
        Self {
            graph: Graph::new(),
            index: HashMap::new(),
            base: None,
        }
    }

    /// Interns a new asset: returns the existing id if the token is already known
    /// otherwise inserts it
    pub fn asset(&mut self, node: AssetNode) -> NodeId {
        let token = node.token;
        if let Some(&id) = self.index.get(&token) {
            return id;
        }
        let id = self.graph.add_node(node);
        self.index.insert(token, id);
        id
    }

    pub fn lookup(&self, token: &Address) -> Option<NodeId> {
        return self.index.get(token).copied();
    }
    
    pub fn token_of(&self, node_id: NodeId) -> Option<Address> {
        self.graph.get_node(node_id).map(| n | n.token)
    }

    /// Marks the base asset (typically WETH). Revenue is denominated here, and
    /// a cycle that doesn't touch it needs a connecting market on the way out.
    pub fn set_base(&mut self, token: Address, decimals: u8, symbol: impl Into<String>) -> NodeId {
        let id = self.asset(AssetNode::new(token, decimals, symbol));
        self.base = Some(id);
        id
    }
    
    pub fn base(&self) -> Option<NodeId> {
        self.base
    }

    pub fn add_market(&mut self, from: NodeId, to: NodeId, mut edge: MarketEdge) {
        edge.decimals_in = self
            .graph
            .get_node(from) 
            .expect("Source asset must exist")
            .decimals;
        edge.decimals_out = self 
            .graph
            .get_node(to) 
            .expect("target asset must exist")
            .decimals;
        edge.refresh_weight();
        self.graph.add_edge(from, to, edge);
    }

    /// Convenience: register both tokens of a UniV2 pool and wire both
    /// directions. Returns `(node_a, node_b)`.
    pub fn add_univ2_pool(
        &mut self,
        pool: Address,
        a: AssetNode,
        b: AssetNode,
        reserve_a: U256,
        reserve_b: U256
    ) -> (NodeId, NodeId) {
        let (ta, da) = (a.token, a.decimals);
        let (tb, db) = (b.token, b.decimals);
        let na = self.asset(a);
        let nb = self.asset(b);

        self.add_market(na, nb, 
            MarketEdge::new(
                pool, Venue::UniV2,
                ta, 
                tb, 
                reserve_a, 
                reserve_b, 
                da, 
                db, 
                UNI_V2_FEE_BPS,
            )
        );
        self.add_market(nb, na,
            MarketEdge::new(
                pool, Venue::UniV2,
                tb, 
                ta, 
                reserve_b, 
                reserve_a, 
                db, 
                da, 
                UNI_V2_FEE_BPS,
            )
        );
        (na, nb)
    }

    /// Recomputes every cached weight. Call after bulk reserve updates from a
    /// new block; `apply_swap` already handles the single-edge case.
    pub fn refresh_weight(&mut self) {
        for (_, e) in self.graph.edges_mut() {
            e.data.refresh_weight();
        }
    }

    /// Cheapest (lowest-weight) edge for an ordered pair, as an index into the
    /// source's adjacency list. Collapses parallel markets to the
    /// best price without discarding the others.
    pub fn best_market(&self, from: NodeId, to: NodeId) -> Option<usize> {
        self.graph
            .get_neighbors(from)?
            .iter()
            .enumerate()
            .filter(|(_, e)| e.target == to && e.data.is_quotable())
            .min_by(|(_, x), (_, y)| x.data.weight.total_cmp(&y.data.weight))
            .map(|(i, _)| i)
    }

    /// Sum of edge weights along `nodes` using the best market at each hop.
    /// Negative means arbitrage. `None` if any hop has no quotable market.
    pub fn cycle_weight(&self, nodes: &[NodeId]) -> Option<f64> {
        let mut sum = 0.0;
        for w in nodes.windows(2) {
            let idx = self.best_market(w[0], w[1])?;
            sum += self.graph.edge_at(w[0], idx)?.data.weight;
        }
        Some(sum)
    }
}

impl Default for MarketGraph {
    fn default() -> Self {
        Self::new()
    }
}

// Tests
#[cfg(test)]
mod tests {
    use super::*;
 
    fn addr(b: u8) -> Address {
        Address::repeat_byte(b)
    }
 
    fn weth() -> AssetNode {
        AssetNode::new(addr(0x11), 18, "WETH")
    }
 
    fn usdc() -> AssetNode {
        AssetNode::new(addr(0x22), 6, "USDC")
    }
 
    fn eth(n: u64) -> U256 {
        U256::from(n) * U256::from(10u64).pow(U256::from(18))
    }
 
    fn usd(n: u64) -> U256 {
        U256::from(n) * U256::from(10u64).pow(U256::from(6))
    }
 
    #[test]
    fn asset_interning_is_idempotent() {
        let mut g = MarketGraph::new();
        let a = g.asset(weth());
        let b = g.asset(weth());
        assert_eq!(a, b);
        assert_eq!(g.graph.node_count(), 1);
    }
 
    #[test]
    fn spot_price_normalises_decimals() {
        let mut g = MarketGraph::new();
        // 1000 WETH : 3,000,000 USDC  ->  ~3000 USDC per WETH
        let (nw, _) = g.add_univ2_pool(addr(0xaa), weth(), usdc(), eth(1000), usd(3_000_000));
        let idx = g.best_market(nw, g.lookup(&addr(0x22)).unwrap()).unwrap();
        let p = g.graph.edge_at(nw, idx).unwrap().data.spot_price();
        assert!((p - 2991.0).abs() < 1.0, "got {p}"); // 3000 * 0.997
    }
 
    #[test]
    fn round_trip_through_one_pool_is_a_loss() {
        let mut g = MarketGraph::new();
        let (nw, nu) = g.add_univ2_pool(addr(0xaa), weth(), usdc(), eth(1000), usd(3_000_000));
        // Selling and immediately buying back on the same market: fees only.
        let w = g.cycle_weight(&[nw, nu, nw]).unwrap();
        assert!(w > 0.0, "same-market round trip must be positive, got {w}");
    }
 
    #[test]
    fn mispriced_second_pool_yields_negative_cycle() {
        let mut g = MarketGraph::new();
        let (nw, nu) = g.add_univ2_pool(addr(0xaa), weth(), usdc(), eth(1000), usd(3_000_000));
        // Second pool prices ETH 10% higher; sell there, buy back cheap.
        g.add_univ2_pool(addr(0xbb), weth(), usdc(), eth(1000), usd(3_300_000));
        let w = g.cycle_weight(&[nw, nu, nw]).unwrap();
        assert!(w < 0.0, "expected arbitrage, got {w}");
    }
 
    #[test]
    fn apply_swap_moves_price_against_you() {
        let mut e = MarketEdge::new(
            addr(0xaa),
            Venue::UniV2,
            addr(0x11),
            addr(0x22),
            eth(1000),
            usd(3_000_000),
            18,
            6,
            30,
        );
        let before = e.weight;
        let out = e.apply_swap(eth(100)).unwrap();
        assert!(out > U256::ZERO);
        assert!(e.weight > before, "weight must rise as the pool is drained");
        assert_eq!(e.reserve_in, eth(1100));
    }
 
    #[test]
    fn u256_to_f64_survives_large_values() {
        assert!(u256_to_f64(U256::MAX).is_finite());
        assert_eq!(u256_to_f64(U256::from(42u64)), 42.0);
    }
 
    #[test]
    fn edges_iterator_sees_every_edge() {
        let mut g = MarketGraph::new();
        g.add_univ2_pool(addr(0xaa), weth(), usdc(), eth(1000), usd(3_000_000));
        assert_eq!(g.graph.edges().count(), 2);
        assert_eq!(g.graph.edge_count(), 2);
    }
 
    #[test]
    fn zero_reserve_edge_is_not_quotable() {
        let e = MarketEdge::new(
            addr(0xaa),
            Venue::UniV2,
            addr(0x11),
            addr(0x22),
            U256::ZERO,
            usd(1),
            18,
            6,
            30,
        );
        assert!(!e.is_quotable());
        assert_eq!(e.weight, f64::INFINITY);
    }
}
 
