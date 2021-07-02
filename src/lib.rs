#![no_std]
extern crate alloc;

#[cfg(feature = "stats")]
mod stats;
#[cfg(test)]
mod unit_tests;

#[cfg(feature = "stats")]
pub use stats::*;

use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Debug;
use core::marker::PhantomData;
use core::ops::Deref;
use core::ops::DerefMut;
use header_vec::{HeaderVec, HeaderVecWeak};
use num_traits::AsPrimitive;
use space::MetricPoint;

#[derive(Debug)]
struct HrcEdge<K> {
    key: K,
    neighbor: HVec<K>,
}

#[derive(Debug)]
struct HrcHeader<K> {
    key: K,
    node: usize,
}

#[derive(Debug)]
struct HVec<K>(HeaderVecWeak<HrcHeader<K>, HrcEdge<K>>);

impl<K> HVec<K> {
    fn weak(&self) -> Self {
        unsafe { Self(self.0.weak()) }
    }

    fn neighbors_mut(&mut self) -> impl Iterator<Item = &mut Self> + '_ {
        self.as_mut_slice()
            .iter_mut()
            .map(|HrcEdge { neighbor, .. }| neighbor)
    }
}

impl<K> HVec<K>
where
    K: MetricPoint,
{
    fn neighbors_distance<'a, D>(&'a self, query: &'a K) -> impl Iterator<Item = (Self, D)> + 'a
    where
        D: Copy + Ord + 'static,
        u64: AsPrimitive<D>,
    {
        self.as_slice()
            .iter()
            .map(move |HrcEdge { key, neighbor }| (neighbor.weak(), query.distance(key).as_()))
    }
}

impl<K> Deref for HVec<K> {
    type Target = HeaderVecWeak<HrcHeader<K>, HrcEdge<K>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<K> DerefMut for HVec<K> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Debug)]
struct HrcNode<K, V> {
    key: K,
    value: V,
    /// Contains the edges of each layer of the graph on which this exists.
    layers: Vec<HeaderVec<HrcHeader<K>, HrcEdge<K>>>,
    /// Forms a linked list through the nodes that creates the freshening order.
    next: usize,
}

impl<K, V> HrcNode<K, V> {
    // fn edges(&self, layer: usize) -> &[(K, usize)] {
    //     &self.edges[layer]
    // }

    // fn edges_mut(&mut self, layer: usize) -> &mut Vec<(K, usize)> {
    //     &mut self.edges[layer]
    // }

    fn layers(&self) -> usize {
        self.layers.len()
    }
}

/// Collection for retrieving entries based on key proximity in a metric space.
///
/// Optional type parameter `D` can be set to a smaller unsigned integer (`u8`, `u16`, `u32`) ONLY
/// if you know that the distance metric cannot overflow this unsigned integer. If it does, then
/// you will have issues. `f32` metric sources can be safely used with `u32`, as only the lower
/// 32 bits of the `u64` is utilized in that case, but `f64` CANNOT be used with anything smaller than `u64`.
/// There is no advantage to using `u128` as the distance metric is produced as `u64`.
/// This parameter DOES affect the performance in benchmarks, though the amount may vary between machines.
/// Smaller integer types will yield better performance, but the difference will likely be less than 25%.
/// On one machine, u64 -> u32 yielded 10-20% performance, but u32 -> u16 yielded less than 1%.
#[derive(Debug)]
pub struct Hrc<K, V, D = u64> {
    /// The nodes of the graph. These nodes internally contain their own edges which form
    /// subgraphs of decreasing size called "layers". The lowest layer contains every node,
    /// while the highest layer contains only one node.
    nodes: Vec<HrcNode<K, V>>,
    /// The node which has been cleaned up/inserted most recently.
    freshest: usize,
    /// Clusters with more items than this are split apart.
    max_cluster_len: usize,
    /// This allows a consistent number to be used for distance storage during usage.
    _phantom: PhantomData<D>,
}

impl<K, V, D> Hrc<K, V, D> {
    /// Creates a new [`Hrc`]. It will be empty and begin with default settings.
    pub fn new() -> Self {
        Self {
            nodes: vec![],
            freshest: 0,
            max_cluster_len: 1024,
            _phantom: PhantomData,
        }
    }

    /// Changes the distance metric type.

    /// Sets the max number of items allowed in a cluster before it is split apart.
    pub fn max_cluster_len(self, max_cluster_len: usize) -> Self {
        Self {
            max_cluster_len,
            ..self
        }
    }

    /// Get the (key, value) pair of a node.
    pub fn get(&self, node: usize) -> Option<(&K, &V)> {
        self.nodes.get(node).map(|node| (&node.key, &node.value))
    }

    /// Get the key of a node.
    pub fn get_key(&self, node: usize) -> Option<&K> {
        self.nodes.get(node).map(|node| &node.key)
    }

    /// Get the value of a node.
    pub fn get_value(&self, node: usize) -> Option<&V> {
        self.nodes.get(node).map(|node| &node.value)
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn histogram(&self) -> Vec<Vec<(usize, usize)>> {
        let mut histograms = vec![];
        for layer in 0.. {
            let mut histogram = vec![];
            for edges in self.nodes.iter().filter_map(|node| {
                if node.layers() > layer {
                    Some(node.layers[layer].len())
                } else {
                    None
                }
            }) {
                match histogram.binary_search_by_key(&edges, |&(search_edges, _)| search_edges) {
                    Ok(pos) => histogram[pos].1 += 1,
                    Err(pos) => histogram.insert(pos, (edges, 1)),
                }
            }
            if histogram.is_empty() {
                break;
            } else {
                histograms.push(histogram);
            }
        }
        histograms
    }

    pub fn simple_representation(&self) -> Vec<Vec<usize>> {
        self.nodes
            .iter()
            .map(|node| {
                node.layers[0]
                    .as_slice()
                    .iter()
                    .map(|HrcEdge { neighbor, .. }| neighbor.node)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    }
}

impl<K, V, D> Hrc<K, V, D>
where
    K: MetricPoint + Clone,
    D: Copy + Ord + 'static,
    u64: AsPrimitive<D>,
{
    fn node_weak(&self, layer: usize, node: usize) -> HVec<K> {
        unsafe { HVec(self.nodes[node].layers[layer].weak()) }
    }

    /// Updates the `HeaderVecWeak` in neighbors of this node.
    fn update_weak(&mut self, mut node: HVec<K>, previous: *const ()) {
        let weak = node.weak();
        for neighbor in node.neighbors_mut() {
            for neighbor_neighbor in neighbor.neighbors_mut() {
                if neighbor_neighbor.is(previous) {
                    *neighbor_neighbor = weak.weak();
                }
            }
        }
    }

    fn add_edge_weak(&mut self, layer: usize, a: &mut HVec<K>, b: &mut HVec<K>) {
        let a_key = a.key.clone();
        let b_key = b.key.clone();

        // Add the edge from a to b.
        let edge = HrcEdge {
            key: b_key,
            neighbor: b.weak(),
        };
        if let Some(previous) = a.push(edge) {
            // Update the strong reference first.
            unsafe {
                self.nodes[a.node].layers[layer].update(a.weak().0);
            }
            // Update the neighbors.
            self.update_weak(a.weak(), previous);
        }

        // Add the edge from b to a.
        let edge = HrcEdge {
            key: a_key,
            neighbor: a.weak(),
        };
        if let Some(previous) = b.push(edge) {
            // Update the strong reference first.
            unsafe {
                self.nodes[b.node].layers[layer].update(b.weak().0);
            }
            // Update the neighbors.
            self.update_weak(b.weak(), previous);
        }
    }

    fn add_edge(&mut self, layer: usize, a: usize, b: usize) {
        self.add_edge_weak(
            layer,
            &mut self.node_weak(layer, a),
            &mut self.node_weak(layer, b),
        );
    }

    fn add_edge_dedup_weak(&mut self, layer: usize, a: &mut HVec<K>, b: &mut HVec<K>) -> bool {
        let b_ptr = b.ptr();
        if !a.neighbors_mut().any(|neighbor| neighbor.is(b_ptr)) {
            self.add_edge_weak(layer, a, b);
            true
        } else {
            false
        }
    }

    fn add_edge_dedup(&mut self, layer: usize, a: usize, b: usize) {
        self.add_edge_dedup_weak(
            layer,
            &mut self.node_weak(layer, a),
            &mut self.node_weak(layer, b),
        );
    }

    /// Searches for the nearest neighbor greedily.
    ///
    /// Returns `(node, distance)`.
    pub fn search(&self, layer: usize, query: &K) -> Option<(usize, D)> {
        if self.is_empty() {
            None
        } else {
            Some(self.search_from(layer, 0, query))
        }
    }

    /// Finds the nearest neighbor to the query key starting from the `from` node using greedy search.
    ///
    /// Returns `(node, distance)`.
    pub fn search_from(&self, layer: usize, from: usize, query: &K) -> (usize, D) {
        // Get the weak node that corresponds to the given node on its particular layer.
        let (weak, distance) = self.search_from_weak(
            self.node_weak(layer, from),
            query.distance(&self.nodes[from].key).as_(),
            query,
        );
        // Get the index from the weak node.
        (weak.node, distance)
    }

    /// Finds the nearest neighbor to the query key starting from the `from` node using greedy search.
    ///
    /// Returns `(node, distance)`.
    fn search_from_weak(&self, from: HVec<K>, from_distance: D, query: &K) -> (HVec<K>, D) {
        let mut best_weak = from;
        let mut best_distance = from_distance;

        while let Some((neighbor_weak, distance)) = best_weak
            .neighbors_distance(query)
            .min_by_key(|(_, distance)| *distance)
        {
            if distance < best_distance {
                best_weak = neighbor_weak.weak();
                best_distance = distance;
            } else {
                break;
            }
        }
        (best_weak, best_distance)
    }

    /// Finds the knn greedily from a starting node `from`.
    ///
    /// Returns (node, distance, searched) pairs. `searched` will always be true, so you can ignore it.
    pub fn search_knn_from(
        &self,
        layer: usize,
        from: usize,
        query: &K,
        num: usize,
    ) -> impl Iterator<Item = (usize, D)> {
        self.search_knn_from_weak(
            self.node_weak(layer, from),
            query.distance(&self.nodes[from].key).as_(),
            query,
            num,
        )
        .into_iter()
        .map(|(weak, distance, _)| (weak.node, distance))
    }

    /// Finds the knn greedily from a starting node `from`.
    ///
    /// Returns (node, distance, searched) pairs. `searched` will always be true, so you can ignore it.
    fn search_knn_from_weak(
        &self,
        from: HVec<K>,
        from_distance: D,
        query: &K,
        num: usize,
    ) -> Vec<(HVec<K>, D, bool)> {
        if num == 0 {
            return vec![];
        }
        // Perform a greedy search first to save time.
        let (from, from_distance) = self.search_from_weak(from, from_distance, query);
        // Contains the index and the distance as a pair.
        let mut bests = vec![(from, from_distance, false)];

        loop {
            if let Some((previous_node, _, searched)) =
                bests.iter_mut().find(|&&mut (_, _, searched)| !searched)
            {
                // Set this as searched (we are searching it now).
                *searched = true;
                // Erase the reference to the search node (to avoid lifetime & borrowing issues).
                let previous_node = previous_node.weak();
                for HrcEdge { key, neighbor } in previous_node.as_slice() {
                    // Make sure that we don't have a copy of this node already or we will get duplicates.
                    if bests.iter().any(|(node, _, _)| neighbor.is(node.ptr())) {
                        continue;
                    }

                    // Compute the distance from the query.
                    let distance = query.distance(key).as_();
                    // If we dont have enough yet, add it.
                    if bests.len() < num {
                        bests.insert(
                            bests.partition_point(|&(_, best_distance, _)| {
                                best_distance <= distance
                            }),
                            (neighbor.weak(), distance, false),
                        );
                    } else if distance < bests.last().unwrap().1 {
                        // Otherwise only add it if its better than the worst item we have.
                        bests.pop();
                        bests.insert(
                            bests.partition_point(|&(_, best_distance, _)| {
                                best_distance <= distance
                            }),
                            (neighbor.weak(), distance, false),
                        );
                    }
                }
            } else {
                return bests;
            }
        }
    }

    /// Finds the knn of `node` greedily.
    pub fn search_knn_of(
        &self,
        layer: usize,
        node: usize,
        num: usize,
    ) -> impl Iterator<Item = (usize, D)> {
        self.search_knn_from(layer, node, &self.nodes[node].key, num)
    }

    /// Finds the knn of `node` greedily.
    fn search_knn_of_weak(&self, node: HVec<K>, num: usize) -> Vec<(HVec<K>, D, bool)> {
        let key = &node.key;
        self.search_knn_from_weak(node.weak(), 0.as_(), key, num)
    }

    /// Insert a (key, value) pair.
    ///
    /// This connects the node to its greedy nearest neighbor. It is recommended to optimize the node
    /// further using some method.
    pub fn insert(&mut self, layer: usize, key: K, value: V, quality: usize) -> usize {
        // Add the node (it will be added this way regardless).
        let node = self.nodes.len();
        // Create the node.
        // The current freshest node's `next` is the stalest node, which will subsequently become
        // the freshest when freshened. If this is the only node, looking up the freshest node will fail.
        // Due to that, we set this node's next to itself if its the only node.
        let node_header_vec = HeaderVec::new(HrcHeader {
            key: key.clone(),
            node,
        });
        self.nodes.push(HrcNode {
            key: key.clone(),
            value,
            layers: vec![node_header_vec],
            next: if node == 0 {
                0
            } else {
                self.nodes[self.freshest].next
            },
        });
        // The previous freshest node should now be freshened right before this node, as this node is now fresher.
        // Even if this is the only node, this will still work because this node still comes after itself in the freshening order.
        self.nodes[self.freshest].next = node;
        // This is now the freshest node.
        self.freshest = node;

        if node == 0 {
            return 0;
        }

        // Find nearest neighbor via greedy search.
        let nn = self
            .search_knn_from(layer, 0, &key, quality)
            .next()
            .unwrap()
            .0;

        // Add edge to nearest neighbor.
        self.add_edge(layer, nn, node);

        node
    }

    /// Freshens up the stalest node by pruning as many edges as reasonably possible from it.
    /// This puts the node into a state where its local neighborhood is probably not optimized well,
    /// and it might form a local minima for some path that passes nearby.
    ///
    /// If you run this function, it is recommended to run [`Hrc::optimize_connection`] on this node,
    /// its neighbors, and other various nodes (can be random).
    ///
    /// Returns the freshened node or `None` if the HRC was empty.
    pub fn freshen(&mut self) -> Option<usize> {
        if !self.is_empty() {
            // The freshest node's next is the stalest node.
            let node = self.nodes[self.freshest].next;
            // The linked list through the nodes remains the same, we only move the freshest forward by 1.
            self.freshest = node;
            // Freshen the node.
            self.freshen_node(node);
            Some(node)
        } else {
            None
        }
    }

    /// Freshens all nodes. See [`Hrc::freshen`]. This does not update the freshening order.
    ///
    /// It is recommended to use [`Hrc::freshen`] instead of this method.
    pub fn freshen_all(&mut self) {
        for node in 0..self.len() {
            self.freshen_node(node);
        }
    }

    /// Freshens up a particular node. See [`Hrc::freshen`]. This does not update the freshening order.
    ///
    /// It is recommended to use [`Hrc::freshen`] instead of this method.
    pub fn freshen_node(&mut self, node: usize) {
        // TODO: See if the layer of this node can be lowered as an optimization before reinserting it on all layers.
        // Reinsert the node on all layers to freshen it.
        for layer in 0..self.nodes[node].layers() {
            self.reinsert(layer, node);
        }
    }

    /// Removes a node from the graph and then reinserts it with the minimum number of edges on a particular layer.
    ///
    /// It is recommended to use [`Hrc::freshen`] instead of this method.
    pub fn reinsert(&mut self, layer: usize, node: usize) {
        // This wont work if we only have 1 node.
        if self.len() == 1 {
            return;
        }

        let mut node = self.node_weak(layer, node);
        let node_key = node.key.clone();

        // Disconnect the node.
        let neighbors = self.disconnect(&mut node);

        // Make sure each neighbor can connect greedily to prevent disconnected graphs.
        for (neighbor, distance) in neighbors {
            let (mut nn, _) =
                self.search_from_weak(self.node_weak(layer, neighbor), distance, &node_key);
            if !nn.is(node.ptr()) {
                self.add_edge_dedup_weak(layer, &mut nn, &mut node);
            }
        }
    }

    /// Internal function for disconnecting a node from the graph.
    ///
    /// Returns nodes as usize because as nodes are re-added, it is possible that neighbors reallocate
    /// and break the weak pointers.
    ///
    /// Returns (node, distance) pairs.
    fn disconnect(&mut self, node: &mut HVec<K>) -> Vec<(usize, D)> {
        let node_key = node.key.clone();
        let mut neighbors = vec![];
        let ptr = node.ptr();
        for (mut neighbor, distance) in node.neighbors_distance(&node_key) {
            neighbor.retain(|HrcEdge { neighbor, .. }| !neighbor.is(ptr));
            neighbors.push((neighbor.node, distance));
        }
        node.retain(|_| false);
        neighbors
    }

    /// Trains by creating optimized greedy search paths from `quality` nearest neighbors towards the key.
    pub fn train(&mut self, layer: usize, key: &K, quality: usize) {
        if self.nodes.len() >= 2 {
            // First, we want to find `quality` nearest neighbors to the key.
            let mut knn = self.search_knn_from(layer, 0, key, quality);
            let mut best_distance = knn.next().unwrap().1;
            // Make sure that there is a greedy search path from all found nearest neighbors to the key.
            // We set the termination distance at the found nearest neighbor's distance (the closest known distance).
            for (nn, _) in knn {
                // Update the distance on each iteration in case we get a better distance.
                best_distance = self
                    .optimize_target_directed(layer, nn, best_distance, key)
                    .1;
            }
        }
    }

    /// Optimizes the connection between two nodes to ensure the optimal greedy search path is available in both directions.
    ///
    /// This works even if the two nodes exist in totally disconnected graphs.
    pub fn optimize_connection(&mut self, layer: usize, a: usize, b: usize) {
        self.optimize_connection_directed(layer, a, b);
        self.optimize_connection_directed(layer, b, a);
    }

    pub fn optimize_connection_directed(&mut self, layer: usize, from: usize, to: usize) {
        if from == to {
            return;
        }
        let key = self.nodes[to].key.clone();
        let (found, distance) = self.optimize_target_directed(layer, from, 0.as_(), &key);
        // Check if we didnt find the target node.
        if found != to {
            // Check if we just found a colocated node.
            if distance == 0.as_() {
                // In that case just make sure they are connected.
                self.add_edge_dedup(layer, found, to);
            } else {
                panic!(
                    "fatal; graph is disconnected: {:?}",
                    self.simple_representation()
                );
            }
        }
    }

    /// Ensures that the optimal greedy path exists towards a specific key from a specific node.
    ///
    /// Will terminate when a distance equal to or better than `to_distance` is reached.
    ///
    /// Returns the termination node.
    pub fn optimize_target_directed(
        &mut self,
        layer: usize,
        from: usize,
        min_distance: D,
        target: &K,
    ) -> (usize, D) {
        let (weak, distance) = self.optimize_target_directed_weak(
            layer,
            self.node_weak(layer, from),
            self.nodes[from].key.distance(target).as_(),
            min_distance,
            target,
        );
        (weak.node, distance)
    }

    /// Ensures that the optimal greedy path exists towards a specific key from a specific node.
    ///
    /// Will terminate when a distance equal to or better than `to_distance` is reached.
    ///
    /// Returns the termination node.
    fn optimize_target_directed_weak(
        &mut self,
        layer: usize,
        from: HVec<K>,
        from_distance: D,
        min_distance: D,
        target: &K,
    ) -> (HVec<K>, D) {
        // Search towards the target greedily.

        let (mut from, mut from_distance) = self.search_from_weak(from, from_distance, target);

        // This loop will gradually break through local minima using the nearest neighbor possible repeatedly
        // until a greedy search path is established.
        'outer: loop {
            // Check if we matched or exceeded expectations.
            if from_distance <= min_distance || from.is_empty() {
                return (from, from_distance);
            }

            // In any other case, we have hit a local (but not global) minima.
            // Our goal is to find the nearest neighbor which can break through the local minima.
            // This process will be tried with exponentially more nearest neighbors until
            // we find the nearest neighbor that can break through the minima.
            // We start with a specific quality so that we are more likely to get the true nearest neighbors
            // than if we just started with 2.
            for quality in core::iter::successors(Some(from.len().saturating_mul(2)), |&quality| {
                if quality >= self.len() {
                    None
                } else {
                    Some(quality.saturating_mul(2))
                }
            }) {
                // Start by finding the nearest neighbors to the local minima starting at itself.
                let knn = self.search_knn_of_weak(from.weak(), quality);

                // Go through the nearest neighbors in order from best to worst.
                for (mut nn, _, _) in knn.into_iter().skip(1) {
                    // Compute the distance to the target from the nn.
                    let nn_distance = nn.key.distance(target).as_();
                    // Check if this node is closer to the target than `from`.
                    if nn_distance < from_distance {
                        // In this case, a greedy search to this node would get closer to the target,
                        // so add an edge to this node. This will update the weak ref if necessary.
                        self.add_edge_weak(layer, &mut nn, &mut from);
                        // Then we need to perform a greedy search towards the target from this node.
                        // This will become the new node for the next round of the loop.
                        let (new_from, new_from_distance) =
                            self.search_from_weak(nn, nn_distance, target);
                        from = new_from;
                        from_distance = new_from_distance;
                        // Continue the outer loop to iteratively move towards the target.
                        continue 'outer;
                    }
                }
            }
            // If we get to this point, we searched the entire graph and there was no path.
            return (from, from_distance);
        }
    }

    /// This optimizes the node's connection to the `quality` most recently added nodes.
    pub fn optimize_recents(&mut self, layer: usize, node: usize, quality: usize) {
        // Use quality latest nodes to optimize graph with node.
        for other in self.len() - core::cmp::min(quality, self.len())..self.len() {
            self.optimize_connection(layer, node, other);
        }
    }

    /// Optimizes a node by discovering local minima, and then breaking through all the local minima
    /// to the closest neighbor which is closer to the target.
    pub fn optimize_local(
        &mut self,
        layer: usize,
        node: usize,
        targets: impl IntoIterator<Item = usize>,
    ) {
        if self.len() == 1 {
            return;
        }
        let mut node = self.node_weak(layer, node);
        let mut knn = self
            .search_knn_of_weak(node.weak(), node.len().saturating_mul(2))
            .into_iter()
            .skip(1)
            .map(|(nn, _, _)| (nn.node, nn.key.clone()))
            .collect();
        let mut neighbors = node
            .as_slice()
            .iter()
            .map(|HrcEdge { key, .. }| key.clone())
            .collect();
        for target in targets {
            if target == node.node {
                continue;
            }
            self.optimize_local_weak(layer, &mut node, target, &mut knn, &mut neighbors)
        }
    }

    /// Optimizes a node by discovering local minima, and then breaking through all the local minima
    /// to the closest neighbor which is closer to the target.
    fn optimize_local_weak(
        &mut self,
        layer: usize,
        node: &mut HVec<K>,
        target_node: usize,
        knn: &mut Vec<(usize, K)>,
        neighbors: &mut Vec<K>,
    ) {
        // Get the target.
        let target_key = self.nodes[target_node].key.clone();
        // Get this node's distance.
        let this_distance = node.key.distance(&target_key).as_();
        // If this node is colocated with the target, connect them.
        if this_distance == 0.as_() {
            if self.add_edge_dedup_weak(layer, node, &mut self.node_weak(layer, target_node)) {
                neighbors.push(target_key);
            }
            return;
        }
        // Get all this node's neighbor's distances, and see if any of them are better.
        if neighbors
            .iter()
            .any(|key| key.distance(&target_key).as_() < this_distance)
        {
            // If any of them are better, no optimization is needed.
            return;
        }

        // Otherwise, lets find the closest neighbor to `node` that is closer to `target` than `node` is.
        loop {
            // Go through the nearest neighbors in order from best to worst.
            for (nn, nn_key) in knn.iter().cloned() {
                // Compute the distance to the target from the nn.
                let nn_distance = nn_key.distance(&target_key).as_();
                // Check if this node is closer to the target than `from`.
                if nn_distance < this_distance {
                    // In this case, a greedy search to this node would get closer to the target,
                    // so add an edge to this node. This will update the weak ref if necessary.
                    neighbors.push(nn_key);
                    self.add_edge_weak(layer, &mut self.node_weak(layer, nn), node);
                    // The greedy path now exists, so exit.
                    return;
                }
            }

            let before_len = knn.len();
            // If the knn is at the full graph and we just did a search, then we couldnt find a better node in the entire graph, including the target itself.
            if before_len + 1 == self.len() {
                panic!(
                    "fatal; searched entire graph: {:?}",
                    self.simple_representation()
                );
            }
            // Double the knn space.
            *knn = self
                .search_knn_of_weak(node.weak(), (before_len + 1).saturating_mul(2))
                .into_iter()
                .skip(1)
                .map(|(nn, _, _)| (nn.node, nn.key.clone()))
                .collect();
            // If the knn didnt grow yet we didnt reach the full graph size, the graph is disconnected.
            if before_len == knn.len() {
                panic!(
                    "fatal; graph is disconnected: {:?}, knn: {:?}",
                    self.simple_representation(),
                    knn.iter().map(|&(nn, _)| nn).collect::<Vec<_>>()
                );
            }
        }
    }

    /// Computes the distance between two nodes.
    pub fn distance(&self, a: usize, b: usize) -> D {
        self.nodes[a].key.distance(&self.nodes[b].key).as_()
    }
}

impl<K, V, D> Default for Hrc<K, V, D> {
    fn default() -> Self {
        Self::new()
    }
}
