use std::collections::HashMap;

use alloy_primitives::{Address, U256};

use crate::common::graph::{MarketEdge, MarketGraph, NodeId};

/// Float stack for negative-cycle detection. Weights are `-ln(price)`; a real
/// arb clears this by orders of magnitude, while a same pool round trip sits at
/// `+2 * -ln(1 - fee) ≈ +0.006`. So EPS will reject float noise without dropping
/// arb worth gas.
const EPS: f64 = 1e-9;

/// One leg of an arbitrage: swap `token_in` for `token_out` in pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hop {
    pub pool: Address,
    pub token_in: Address,
    pub token_out: Address,
}

#[derive(Debug, Clone)]
pub struct ArbitrageOpportunity {
    /// Legs in execution order; `hops.last().token_out == entry token`.
    pub hops: Vec<Hop>,
    /// The token the cycle enters and exits on.
    pub entry_token: Address,
    /// Profit maximizing input, in `entry_token`'s smallest units.
    pub amount_in: U256,
    /// Output after the full cycle in `entry_token`'s smallest units.
    pub amount_out: U256,
    /// `amount_out - amount_in` in `entry_token`'s smallest units
    pub profit: U256,
    /// Profit in *whole base tokens*, spot converted from `entry_token`.
    /// `None` if no base is set and no connecting markets exist
    pub profit_base: Option<f64>,
    /// Cycle weight at detection (negative). 
    pub weight_sum: f64
}

const MAX_OPPORTUNITIES: usize = 16;
#[derive(Debug, Clone)]
pub struct ArbConfig {
    /// Cap on cycles examined per `find_arbitrage` call — bounds work so the
    /// pass fits inside block time.
    pub max_opportunities: usize,
    /// Keep only opportunities worth at least this many whole base tokens.
    /// Applies to `profit_base`; opportunities with no base conversion fall
    /// back to "profit > 0".
    pub min_profit_base: f64,    
}

impl Default for ArbConfig {
    fn default() -> Self {
        Self {
            max_opportunities: MAX_OPPORTUNITIES,
            min_profit_base: 0.0
        }
    }
}

/// Find the best arbitrage in the current graph state without mutating it.
/// Returns `None` if there is no negative cycle or at an unprofitable size.
pub fn detect_arbitrage(g: &MarketGraph) -> Option<ArbitrageOpportunity> {
    let mut edges = find_negative_cycle(g)?;
    rotate_to_base(g, &mut edges);
    let (amount_in, amount_out) = optimize_amount_in(g, &edges);
    if amount_in.is_zero() || amount_out <= amount_in {
        return None;
    }
    build_opportunity(g, &edges, amount_in, amount_out)
}

/// Greedy multi-extraction. Detects a cycle, weighs it, and applies it to the graph--neutralizing 
/// the cycle, revealing any serquential ones--and repeats
pub fn find_arbitrage(g: &mut MarketGraph, cfg: &ArbConfig) -> Vec<ArbitrageOpportunity> {
    let pool_map = build_pool_map(g);
    let mut found = Vec::new();

    for _ in 0..cfg.max_opportunities {
        let Some(mut edges) = find_negative_cycle(g) else {
            break;
        };
        rotate_to_base(g, &mut edges);
        let (amount_in, amount_out) = optimize_amount_in(g, &edges);

        if amount_in.is_zero() || amount_out <= amount_in {
            break;
        }

        let Some(opp) = build_opportunity(g, &edges, amount_in, amount_out) else{
            break;
        };

        let keep = match opp.profit_base {
            Some(pb) => pb >= cfg.min_profit_base,
            None => !opp.profit.is_zero(),
        };

        // Apply regardless: executing at the optimum flattens this cycle so the
        // next pass finds a *different* one instead of looping on this one.
        if execute_cycle(g, &edges, &pool_map, amount_in).is_none() {
            break;
        }

        if keep {
            found.push(opp);
        }
    }

    found

}


// Bellman-Ford-Moore + walk-to-the-root

/// Detects a negative cycle and returns its edges as `(source_node, edge_index)`
/// pairs in execution order, closing back on the first source. Edge indices are 
/// positions in the source node's adjacency list. The pools, not the token pairs.
fn find_negative_cycle(g: &MarketGraph) -> Option<Vec<(NodeId, usize)>> {
    let n = g.graph.node_count();
    if n == 0 {
        return None;
    }

    let mut dist = vec![0.0f64; n];
    let mut pred: Vec<Option<(NodeId, usize)>> = vec![None; n];

    // |N| - 1 relaxation passes, with early exit on convergence.
    for _ in 0..n.saturating_sub(1) {
        let mut changed = false;
        for src in 0..n {
            let ds = dist[src];
            if ds.is_infinite() {
                continue;
            }
            let Some(neighbors) = g.graph.get_neighbors(src) else {
                continue;
            };
            for (idx, e) in neighbors.iter().enumerate() {
                let w = e.data.weight;
                if w.is_infinite() {
                    continue;
                }
                if ds + w + EPS < dist[e.target] {
                    dist[e.target] = ds + w;
                    pred[e.target] = Some((src, idx));
                    changed = true;
                }
            }
        }

        if !changed {
            return None; // converged ⇒ no negative cycle
        }
    }

    // Detection pass: any edge that still relaxes hangs off a negative cycle.
    let mut hit = None;
    'scan: for src in 0..n {
        let ds = dist[src];
        if ds.is_infinite() {
            continue;
        }
        let Some(neighbors) = g.graph.get_neighbors(src) else {
            continue;
        };
        for (idx, e) in neighbors.iter().enumerate() {
            let w = e.data.weight;
            if w.is_infinite() {
                continue;
            }
            if ds + w + EPS < dist[e.target] {
                pred[e.target] = Some((src, idx));
                hit = Some(e.target);
                break 'scan;
            }
        }
    }

    let start = hit?;

    // The relaxing node is *reachable from* the cycle but may not be *on* it.
    // Following predecessors n times is guaranteed to land inside the cycle.
    let mut cursor = start;
    for _ in 0..n {
        cursor = pred[cursor]?.0;
    }
    let anchor = cursor;

    // Walk the predecessor chain once around the cycle, collecting edges.
    let mut edges_rev = Vec::new();
    let mut cur = anchor;
    loop {
        let (pnode, pidx) = pred[cur]?;
        edges_rev.push((pnode, pidx)); 
        cur = pnode;
        if cur == anchor {
            break;
        }
        if edges_rev.len() > n {
            return None;
        }

    }

    edges_rev.reverse();
    Some(edges_rev)
} 

// Parameter search

/// Threads `amount_in` through the cycle against current reserves, without mutating 
pub fn quote_cycle(g: &MarketGraph, edges: &Vec<(NodeId, usize)>, amount_in: U256) -> Option<U256> {
    let mut amount = amount_in;
    for &(src, idx) in edges {
        let edge = g.graph.edge_at(src, idx)?;
        amount = edge.data.amount_out(amount)?;
        if amount.is_zero() {
            return Some(U256::ZERO);
        }
    }
    Some(amount)
}

/// Profit `out(x) - x` is concave, with `profit(0) - 0` so ternary search finds
/// the maximizing inputs.
pub fn optimize_amount_in(g: &MarketGraph, edges: &Vec<(NodeId, usize)>) -> (U256, U256) {
    let Some(first) = g.graph.edge_at(edges[0].0, edges[0].1) else{
        return (U256::ZERO, U256::ZERO);
    };
    let cap = first.data.reserve_in;
    if cap.is_zero() {
        return (U256::ZERO, U256::ZERO);
    }

    let out_at = |x: U256| quote_cycle(g, &edges, x).unwrap_or(U256::ZERO);

    let two = U256::from(2u8);
    let three =  U256::from(3u8);
    let mut lo = U256::ZERO;
    let mut hi = cap;

    // Compare profit(a) vs profit(b) as `out(a) + b` vs `out(b) + a` - all U256
    while hi - lo > two {
        let third = (hi - lo) / three;
        let m1 = lo + third;
        let m2 = hi - third;
        if out_at(m1) + m2 < out_at(m2) + m1 {
            lo = m1;
        }
        else {
            hi = m2;
        }
    }

    // Exact scan of the final ≤3-wide window.
    let mut best_x = lo;
    let mut best_out = out_at(lo);
    let mut x = lo;
    while x <= hi {
        let out = out_at(x);
        if out + best_x > best_out + x {
            best_out = out;
            best_x = x;
        }
        if x == hi {
            break;
        }
        x += U256::from(1u8);
    }

    (best_x, best_out)

}

// Execution into model

/// `pool address -> every (node, edge_index) that pool appears at`.
fn build_pool_map(g: &MarketGraph) -> HashMap<Address, Vec<(NodeId, usize)>> {
    let mut m: HashMap<Address, Vec<(NodeId, usize)>> = HashMap::new();
    for src in 0..g.graph.node_count() {
        if let Some(neighbors) = g.graph.get_neighbors(src) {
            for (idx, e) in neighbors.iter().enumerate() {
                m.entry(e.data.pool).or_default().push((src, idx));
            }
        }
    }
    m
}

/// Applies the cycle to the graph, threading the amount hop-to-hop. For each
/// pool it swaps the forward edge and mirrors the result into the reverse edge.
fn execute_cycle(
    g: &mut MarketGraph,
    edges: &Vec<(NodeId, usize)>,
    pool_map: &HashMap<Address, Vec<(NodeId, usize)>>,
    amount_in: U256,
) -> Option<U256> {
    let mut amount = amount_in;
    for &(src, idx) in edges {
        let (pool_address, out, fin, fout) = {
            let e = g.graph.edge_at_mut(src, idx)?;
            let o = e.data.apply_swap(amount)?;
            (e.data.pool, o, e.data.reserve_in, e.data.reserve_out)
        };
        amount = out;

        if let Some(locs) = pool_map.get(&pool_address) {
            if let Some(&(rs, ri)) = locs.iter().find(|&&loc| loc != (src, idx)) {
                if let Some(rev) = g.graph.edge_at_mut(rs, ri) {
                    // Reverse direction: reserves are the same two numbers, swapped.
                    rev.data.reserve_in = fout;
                    rev.data.reserve_out = fin;
                    rev.data.refresh_weight();
                }
            }
        }
    }
    Some(amount)
}

// Reporting

pub fn build_opportunity(
    g: &MarketGraph, 
    edges: &Vec<(NodeId, usize)>, 
    amount_in: U256, 
    amount_out: U256
) -> Option<ArbitrageOpportunity> {
    let mut hops = Vec::with_capacity(edges.len());
    let mut weight_sum = 0.0;
    for &(src, idx) in edges {
        let e = g.graph.edge_at(src, idx)?;
        weight_sum += e.data.weight;
        hops.push(Hop {
            pool: e.data.pool,
            token_in: g.token_of(src)?,
            token_out: g.token_of(e.target)?,
        });
    }

    let entry_node = edges[0].0;
    let entry_token = g.token_of(entry_node)?;
    let profit = amount_out.saturating_sub(amount_in);
    let profit_base = base_value_whole(g, entry_node, profit);

    Some(ArbitrageOpportunity {
        hops,
        entry_token,
        amount_in,
        amount_out,
        profit,
        profit_base,
        weight_sum,
    })
}

fn base_value_whole(g: &MarketGraph, token_node: NodeId, amount: U256) -> Option<f64> {
    let base = g.base()?;
    let token_dec = g.graph.get_node(token_node)?.decimals;
    let amt_whole = u256_to_f64(amount) / 10f64.powi(token_dec as i32);

    if token_node == base {
        return Some(amt_whole);
    }

    let idx = g.best_market(token_node, base)?;
    let px = g.graph.edge_at(token_node, idx)?.data.spot_price();
    Some(amt_whole * px)
}

/// Local mirror of graph.rs's helper (private there). Loses precision above
/// ~2^53 of significand — fine for the f64 triage figure, never used for
/// settlement, which stays in `U256`.
fn u256_to_f64(v: U256) -> f64 {
    let mut out = 0.0f64;
    for (i, limb) in v.as_limbs().iter().enumerate() {
        out += (*limb as f64) * 2f64.powi(64 * i as i32);
    }
    out
}

/// Rotates the cycle so it begins at the base asset, giving a directly
/// executable base→…→base loop. No-op if no base is set or the base isn't on
/// this cycle (there, the caller falls back to connecting-market conversion).
fn rotate_to_base(g: &MarketGraph, edges: &mut Vec<(NodeId, usize)>) {
    let Some(base) = g.base() else { return; };
    if let Some(p) = edges.iter().position(|&(src, _)| src == base) {
        edges.rotate_left(p);
    }
}

// Tests
#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::graph::AssetNode;
 
    fn addr(b: u8) -> Address {
        Address::repeat_byte(b)
    }
    fn weth() -> AssetNode {
        AssetNode::new(addr(0x11), 18, "WETH")
    }
    fn usdc() -> AssetNode {
        AssetNode::new(addr(0x22), 6, "USDC")
    }
    fn dai() -> AssetNode {
        AssetNode::new(addr(0x33), 18, "DAI")
    }
    fn eth(n: u64) -> U256 {
        U256::from(n) * U256::from(10u64).pow(U256::from(18))
    }
    fn usd(n: u64) -> U256 {
        U256::from(n) * U256::from(10u64).pow(U256::from(6))
    }
 
    /// One pool ⇒ the only 2-cycle is a same-pool round trip (positive weight).
    /// No arbitrage.
    #[test]
    fn single_pool_has_no_arbitrage() {
        let mut g = MarketGraph::new();
        g.add_univ2_pool(addr(0xaa), weth(), usdc(), eth(1000), usd(3_000_000));
        assert!(detect_arbitrage(&g).is_none());
    }
 
    /// Two pools pricing WETH 10% apart: sell dear, buy cheap.
    #[test]
    fn detects_two_pool_arbitrage() {
        let mut g = MarketGraph::new();
        g.set_base(addr(0x11), 18, "WETH");
        g.add_univ2_pool(addr(0xaa), weth(), usdc(), eth(1000), usd(3_000_000));
        g.add_univ2_pool(addr(0xbb), weth(), usdc(), eth(1000), usd(3_300_000));
 
        let opp = detect_arbitrage(&g).expect("arb exists");
        assert_eq!(opp.hops.len(), 2);
        assert_eq!(opp.entry_token, addr(0x11), "WETH-rooted");
        assert_eq!(opp.hops.last().unwrap().token_out, addr(0x11), "cycle closes");
        assert!(opp.profit > U256::ZERO);
        assert!(opp.amount_out > opp.amount_in);
        assert!(opp.weight_sum < 0.0, "detected as a negative cycle");
    }
 
    #[test]
    fn optimized_input_stays_within_first_pool() {
        let mut g = MarketGraph::new();
        g.add_univ2_pool(addr(0xaa), weth(), usdc(), eth(1000), usd(3_000_000));
        g.add_univ2_pool(addr(0xbb), weth(), usdc(), eth(1000), usd(3_300_000));
        let opp = detect_arbitrage(&g).unwrap();
        assert!(opp.amount_in > U256::ZERO);
        assert!(opp.amount_in < eth(1000), "input below first-hop reserves");
    }
 
    #[test]
    fn profit_base_is_whole_tokens_when_base_in_cycle() {
        let mut g = MarketGraph::new();
        g.set_base(addr(0x11), 18, "WETH");
        g.add_univ2_pool(addr(0xaa), weth(), usdc(), eth(1000), usd(3_000_000));
        g.add_univ2_pool(addr(0xbb), weth(), usdc(), eth(1000), usd(3_300_000));
 
        let opp = detect_arbitrage(&g).unwrap();
        let pb = opp.profit_base.expect("base is set and in the cycle");
        // Whole-WETH profit should match profit/1e18 within float slack.
        let expected = u256_to_f64(opp.profit) / 1e18;
        assert!((pb - expected).abs() < 1e-9, "pb={pb} expected={expected}");
        assert!(pb > 0.0);
    }
 
    /// WETH -> DAI -> USDC -> WETH, mispriced so the triangle profits.
    #[test]
    fn detects_three_pool_triangle() {
        let mut g = MarketGraph::new();
        g.set_base(addr(0x11), 18, "WETH");
        // 3000 USDC/WETH
        g.add_univ2_pool(addr(0xaa), weth(), usdc(), eth(1000), usd(3_000_000));
        // ~1 DAI/USDC
        g.add_univ2_pool(addr(0xbb), usdc(), dai(), usd(3_000_000), eth(3_000_000));
        // 3300 DAI/WETH — the misprice
        g.add_univ2_pool(addr(0xcc), dai(), weth(), eth(3_300_000), eth(1000));
 
        let opp = detect_arbitrage(&g).expect("triangle arb exists");
        assert_eq!(opp.hops.len(), 3);
        assert_eq!(opp.entry_token, addr(0x11));
        assert_eq!(opp.hops.last().unwrap().token_out, addr(0x11));
        assert!(opp.profit > U256::ZERO);
    }
 
    /// After greedy extraction the cycle is neutralized: a fresh detect finds
    /// nothing, and reserves have actually moved. Also guards against looping.
    #[test]
    fn greedy_extracts_then_neutralizes() {
        let mut g = MarketGraph::new();
        g.set_base(addr(0x11), 18, "WETH");
        g.add_univ2_pool(addr(0xaa), weth(), usdc(), eth(1000), usd(3_000_000));
        g.add_univ2_pool(addr(0xbb), weth(), usdc(), eth(1000), usd(3_300_000));
 
        let before = g
            .graph
            .edge_at(0, 0)
            .map(|e| (e.data.reserve_in, e.data.reserve_out))
            .unwrap();
 
        let opps = find_arbitrage(&mut g, &ArbConfig::default());
        assert_eq!(opps.len(), 1, "one arb, extracted once");
        assert!(opps[0].profit > U256::ZERO);
 
        let after = g
            .graph
            .edge_at(0, 0)
            .map(|e| (e.data.reserve_in, e.data.reserve_out))
            .unwrap();
        assert_ne!(before, after, "reserves moved");
        assert!(detect_arbitrage(&g).is_none(), "cycle neutralized");
    }
 
    /// The twin-desync guard: after executing through a pool, its reverse edge
    /// mirrors the forward reserves. If this breaks, the next detection pass
    /// sees two different prices for one pool.
    #[test]
    fn execution_mirrors_reverse_edges() {
        let mut g = MarketGraph::new();
        g.set_base(addr(0x11), 18, "WETH");
        g.add_univ2_pool(addr(0xaa), weth(), usdc(), eth(1000), usd(3_000_000));
        g.add_univ2_pool(addr(0xbb), weth(), usdc(), eth(1000), usd(3_300_000));
 
        let _ = find_arbitrage(&mut g, &ArbConfig::default());
 
        // For every pool, forward.reserve_in == reverse.reserve_out and vice versa.
        let map = build_pool_map(&g);
        for locs in map.values() {
            if locs.len() != 2 {
                continue;
            }
            let (a, b) = (locs[0], locs[1]);
            let ea = g.graph.edge_at(a.0, a.1).unwrap();
            let eb = g.graph.edge_at(b.0, b.1).unwrap();
            assert_eq!(ea.data.reserve_in, eb.data.reserve_out);
            assert_eq!(ea.data.reserve_out, eb.data.reserve_in);
        }
    }
 
    #[test]
    fn min_profit_base_filters() {
        let mut g = MarketGraph::new();
        g.set_base(addr(0x11), 18, "WETH");
        g.add_univ2_pool(addr(0xaa), weth(), usdc(), eth(1000), usd(3_000_000));
        g.add_univ2_pool(addr(0xbb), weth(), usdc(), eth(1000), usd(3_300_000));
 
        let cfg = ArbConfig {
            max_opportunities: 16,
            min_profit_base: 1_000_000.0, // absurd floor
        };
        assert!(find_arbitrage(&mut g, &cfg).is_empty());
    }
}
