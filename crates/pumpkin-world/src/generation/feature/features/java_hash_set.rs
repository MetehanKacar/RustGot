// Ported from horizon (MIT): crates/horizon-world/src/java_hash_map.rs
// Copyright (c) 2026 Ludovic Marie and the Horizon contributors.
// Trimmed to the set operations worldgen needs; the bin, resize and red-black tree logic is
// unchanged.
//! `java.util.HashSet` iteration order, including tree bins: a bin that already holds
//! `TREEIFY_LEN` keys when another is appended grows a table smaller than `MIN_TREE_CAPACITY`
//! and otherwise becomes a red-black tree whose root moves to the front of the bin's list.

use std::cmp::Ordering;

pub trait JavaKey: Eq + Copy {
    fn java_hash(&self) -> i32;
    /// `Comparable` ordering when the key's class implements `Comparable` of itself.
    fn comparable_compare(&self, _other: &Self) -> Option<Ordering> {
        None
    }
    /// Java's `tieBreakOrder`: class name, then identity hash. Never `Equal`.
    fn tie_break(&self, other: &Self) -> Ordering;
}

const NIL: u32 = u32::MAX;
const DEFAULT_CAPACITY: usize = 16;
const MAX_CAPACITY: usize = 1 << 30;
const TREEIFY_LEN: usize = 8;
const MIN_TREE_CAPACITY: usize = 64;
const UNTREEIFY_LEN: usize = 6;

#[derive(Clone, Copy, Debug)]
struct Node<K> {
    key: K,
    /// The spread hash: `h ^ (h >>> 16)`.
    hash: i32,
    next: u32,
}

#[derive(Clone, Copy, Debug)]
struct Links {
    parent: u32,
    left: u32,
    right: u32,
    red: bool,
}

impl Links {
    const UNUSED: Self = Self {
        parent: NIL,
        left: NIL,
        right: NIL,
        red: false,
    };
}

#[derive(Clone, Copy, Debug)]
struct Bucket {
    head: u32,
    /// The root of the bin's red-black tree; NIL for a plain bin.
    root: u32,
}

impl Bucket {
    const EMPTY: Self = Self {
        head: NIL,
        root: NIL,
    };
}

#[derive(Clone, Debug)]
pub struct JavaHashSet<K> {
    /// Removed nodes stay in the slab and are reused through `free`.
    nodes: Vec<Node<K>>,
    free: Vec<u32>,
    /// Tree links by node index; empty until the first tree bin.
    links: Vec<Links>,
    buckets: Vec<Bucket>,
    threshold: usize,
    len: usize,
}

impl<K: JavaKey> Default for JavaHashSet<K> {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            free: Vec::new(),
            links: Vec::new(),
            buckets: Vec::new(),
            threshold: 0,
            len: 0,
        }
    }
}

const fn spread(hash: i32) -> i32 {
    hash ^ ((hash as u32) >> 16) as i32
}

const fn threshold_for(capacity: usize) -> usize {
    if capacity >= MAX_CAPACITY {
        usize::MAX
    } else {
        capacity * 3 / 4
    }
}

impl<K: JavaKey> JavaHashSet<K> {
    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn contains(&self, key: &K) -> bool {
        self.find(key).is_some()
    }

    pub fn iter(&self) -> Iter<'_, K> {
        Iter {
            set: self,
            next: self.first_node_from(0),
        }
    }

    /// `HashSet.add`. Returns whether the key was new.
    pub fn insert(&mut self, key: K) -> bool {
        let hash = spread(key.java_hash());
        if self.buckets.is_empty() {
            self.grow();
        }
        let b = self.bucket_index(hash);
        let Bucket { head, root } = self.buckets[b];
        if head == NIL {
            self.buckets[b].head = self.alloc(key, hash);
        } else if root != NIL {
            if self.tree_find(root, hash, &key) != NIL {
                return false;
            }
            self.tree_put(b, key, hash);
        } else {
            let mut len = 0;
            let mut last = head;
            let mut i = head;
            while i != NIL {
                if self.is_key(i, hash, &key) {
                    return false;
                }
                len += 1;
                last = i;
                i = self.nodes[i as usize].next;
            }
            let new = self.alloc(key, hash);
            self.nodes[last as usize].next = new;
            if len >= TREEIFY_LEN {
                if self.buckets.len() < MIN_TREE_CAPACITY {
                    self.grow();
                } else {
                    self.treeify(b);
                }
            }
        }
        self.len += 1;
        if self.len > self.threshold {
            self.grow();
        }
        true
    }

    /// `iterator().next()` followed by `iterator.remove()`.
    pub fn pop_first(&mut self) -> Option<K> {
        let i = self.first_node_from(0);
        if i == NIL {
            return None;
        }
        let b = self.bucket_index(self.nodes[i as usize].hash);
        Some(self.remove_node(b, i))
    }

    fn alloc(&mut self, key: K, hash: i32) -> u32 {
        let node = Node {
            key,
            hash,
            next: NIL,
        };
        if let Some(i) = self.free.pop() {
            self.nodes[i as usize] = node;
            return i;
        }
        self.nodes.push(node);
        if !self.links.is_empty() {
            self.links.push(Links::UNUSED);
        }
        (self.nodes.len() - 1) as u32
    }

    const fn bucket_index(&self, hash: i32) -> usize {
        hash as u32 as usize & (self.buckets.len() - 1)
    }

    fn is_key(&self, i: u32, hash: i32, key: &K) -> bool {
        let node = &self.nodes[i as usize];
        node.hash == hash && node.key == *key
    }

    fn find(&self, key: &K) -> Option<u32> {
        if self.buckets.is_empty() {
            return None;
        }
        let hash = spread(key.java_hash());
        let Bucket { head, root } = self.buckets[self.bucket_index(hash)];
        let found = if root == NIL {
            let mut i = head;
            while i != NIL && !self.is_key(i, hash, key) {
                i = self.nodes[i as usize].next;
            }
            i
        } else {
            self.tree_find(root, hash, key)
        };
        (found != NIL).then_some(found)
    }

    /// Trees are ordered by hash first; nodes with an equal hash can be on both sides.
    fn tree_find(&self, mut p: u32, hash: i32, key: &K) -> u32 {
        while p != NIL {
            let node = &self.nodes[p as usize];
            let links = self.links[p as usize];
            p = match node.hash.cmp(&hash) {
                Ordering::Greater => links.left,
                Ordering::Less => links.right,
                Ordering::Equal if node.key == *key => return p,
                Ordering::Equal => {
                    let found = self.tree_find(links.right, hash, key);
                    if found != NIL {
                        return found;
                    }
                    links.left
                }
            };
        }
        NIL
    }

    fn goes_left(&self, key: &K, hash: i32, p: u32) -> bool {
        let node = &self.nodes[p as usize];
        match node.hash.cmp(&hash) {
            Ordering::Greater => true,
            Ordering::Less => false,
            Ordering::Equal => match key.comparable_compare(&node.key) {
                Some(Ordering::Less) => true,
                Some(Ordering::Greater) => false,
                _ => key.tie_break(&node.key) != Ordering::Greater,
            },
        }
    }

    /// RB-INSERT of node `i`, then RB-INSERT-FIXUP. Returns the new root and `i`'s parent.
    fn attach(&mut self, root: u32, i: u32) -> (u32, u32) {
        let mut parent = NIL;
        let mut to_left = false;
        let node = self.nodes[i as usize];
        let mut p = root;
        while p != NIL {
            parent = p;
            to_left = self.goes_left(&node.key, node.hash, p);
            let links = self.links[p as usize];
            p = if to_left { links.left } else { links.right };
        }
        self.links[i as usize] = Links {
            parent,
            red: true,
            ..Links::UNUSED
        };
        let mut tree = Tree {
            links: &mut self.links,
            root,
            nil_parent: NIL,
        };
        if parent == NIL {
            tree.root = i;
        } else if to_left {
            tree.at(parent).left = i;
        } else {
            tree.at(parent).right = i;
        }
        tree.insert_fixup(i);
        (tree.root, parent)
    }

    fn treeify(&mut self, b: usize) {
        if self.links.len() < self.nodes.len() {
            self.links.resize(self.nodes.len(), Links::UNUSED);
        }
        let mut root = NIL;
        let mut i = self.buckets[b].head;
        while i != NIL {
            root = self.attach(root, i).0;
            i = self.nodes[i as usize].next;
        }
        self.buckets[b].root = root;
        self.move_root_to_front(b);
    }

    /// A new key joins the tree, and the bin's list right after its tree parent.
    fn tree_put(&mut self, b: usize, key: K, hash: i32) {
        let new = self.alloc(key, hash);
        let (root, parent) = self.attach(self.buckets[b].root, new);
        self.buckets[b].root = root;
        let after = self.nodes[parent as usize].next;
        self.nodes[new as usize].next = after;
        self.nodes[parent as usize].next = new;
        self.move_root_to_front(b);
    }

    fn move_root_to_front(&mut self, b: usize) {
        let Bucket { head, root } = self.buckets[b];
        if root != head {
            self.unlink(b, root);
            self.nodes[root as usize].next = head;
            self.buckets[b].head = root;
        }
    }

    fn unlink(&mut self, b: usize, i: u32) {
        let next = self.nodes[i as usize].next;
        let head = self.buckets[b].head;
        if head == i {
            self.buckets[b].head = next;
            return;
        }
        let mut p = head;
        while self.nodes[p as usize].next != i {
            p = self.nodes[p as usize].next;
        }
        self.nodes[p as usize].next = next;
    }

    /// An iterator's removal: the tree loses the node but is neither untreeified nor reordered.
    fn remove_node(&mut self, b: usize, i: u32) -> K {
        let root = self.buckets[b].root;
        self.unlink(b, i);
        if self.buckets[b].head == NIL {
            self.buckets[b].root = NIL;
        } else if root != NIL {
            let mut tree = Tree {
                links: &mut self.links,
                root,
                nil_parent: NIL,
            };
            tree.delete(i);
            self.buckets[b].root = tree.root;
        }
        self.len -= 1;
        self.free.push(i);
        self.nodes[i as usize].key
    }

    fn successor(&self, i: u32) -> u32 {
        let node = &self.nodes[i as usize];
        if node.next == NIL {
            self.first_node_from(self.bucket_index(node.hash) + 1)
        } else {
            node.next
        }
    }

    fn first_node_from(&self, b: usize) -> u32 {
        self.buckets
            .get(b..)
            .and_then(|buckets| buckets.iter().find(|bucket| bucket.head != NIL))
            .map_or(NIL, |bucket| bucket.head)
    }

    /// Allocates the first table or doubles it; old bucket `j` splits into `j` and `j + old`,
    /// each half in its old order.
    fn grow(&mut self) {
        let old = self.buckets.len();
        if old == 0 {
            self.buckets = vec![Bucket::EMPTY; DEFAULT_CAPACITY];
            self.threshold = threshold_for(DEFAULT_CAPACITY);
            return;
        }
        if old >= MAX_CAPACITY {
            self.threshold = usize::MAX;
            return;
        }
        let old_buckets = std::mem::replace(&mut self.buckets, vec![Bucket::EMPTY; old * 2]);
        for (j, bucket) in old_buckets.into_iter().enumerate() {
            if bucket.head == NIL {
                continue;
            }
            let first = self.nodes[bucket.head as usize];
            if first.next == NIL {
                let to = self.bucket_index(first.hash);
                self.buckets[to] = bucket;
                continue;
            }
            let halves = self.split_list(bucket.head, old);
            for (half, &(head, len)) in halves.iter().enumerate() {
                if head == NIL {
                    continue;
                }
                let to = j + half * old;
                if bucket.root == NIL || len <= UNTREEIFY_LEN {
                    self.buckets[to] = Bucket { head, root: NIL };
                } else if halves[1 - half].0 != NIL {
                    self.buckets[to].head = head;
                    self.treeify(to);
                } else {
                    self.buckets[to] = bucket;
                }
            }
        }
        self.threshold = threshold_for(old * 2);
    }

    fn split_list(&mut self, head: u32, bit: usize) -> [(u32, usize); 2] {
        let mut halves = [(NIL, 0); 2];
        let mut tails = [NIL; 2];
        let mut i = head;
        while i != NIL {
            let node = self.nodes[i as usize];
            let half = usize::from(node.hash as u32 as usize & bit != 0);
            if tails[half] == NIL {
                halves[half].0 = i;
            } else {
                self.nodes[tails[half] as usize].next = i;
            }
            tails[half] = i;
            halves[half].1 += 1;
            i = node.next;
        }
        for tail in tails {
            if tail != NIL {
                self.nodes[tail as usize].next = NIL;
            }
        }
        halves
    }
}

pub struct Iter<'a, K> {
    set: &'a JavaHashSet<K>,
    next: u32,
}

impl<'a, K: JavaKey> Iterator for Iter<'a, K> {
    type Item = &'a K;

    fn next(&mut self) -> Option<&'a K> {
        if self.next == NIL {
            return None;
        }
        let i = self.next;
        self.next = self.set.successor(i);
        Some(&self.set.nodes[i as usize].key)
    }
}

/// One tree bin's red-black tree (CLRS chapter 13). NIL is the black sentinel and `nil_parent`
/// is the `T.nil.p` that RB-DELETE sets for RB-DELETE-FIXUP.
struct Tree<'a> {
    links: &'a mut [Links],
    root: u32,
    nil_parent: u32,
}

impl Tree<'_> {
    const fn at(&mut self, x: u32) -> &mut Links {
        &mut self.links[x as usize]
    }
    const fn left(&self, x: u32) -> u32 {
        self.links[x as usize].left
    }
    const fn right(&self, x: u32) -> u32 {
        self.links[x as usize].right
    }
    const fn parent(&self, x: u32) -> u32 {
        if x == NIL {
            self.nil_parent
        } else {
            self.links[x as usize].parent
        }
    }
    const fn set_parent(&mut self, x: u32, parent: u32) {
        if x == NIL {
            self.nil_parent = parent;
        } else {
            self.at(x).parent = parent;
        }
    }
    const fn is_red(&self, x: u32) -> bool {
        x != NIL && self.links[x as usize].red
    }
    const fn set_red(&mut self, x: u32, red: bool) {
        if x != NIL {
            self.at(x).red = red;
        }
    }
    const fn replace_child(&mut self, parent: u32, old: u32, new: u32) {
        if parent == NIL {
            self.root = new;
        } else if self.left(parent) == old {
            self.at(parent).left = new;
        } else {
            self.at(parent).right = new;
        }
    }
    const fn rotate_left(&mut self, x: u32) {
        let y = self.right(x);
        let beta = self.left(y);
        self.at(x).right = beta;
        if beta != NIL {
            self.at(beta).parent = x;
        }
        let parent = self.parent(x);
        self.at(y).parent = parent;
        self.replace_child(parent, x, y);
        self.at(y).left = x;
        self.at(x).parent = y;
    }
    const fn rotate_right(&mut self, x: u32) {
        let y = self.left(x);
        let beta = self.right(y);
        self.at(x).left = beta;
        if beta != NIL {
            self.at(beta).parent = x;
        }
        let parent = self.parent(x);
        self.at(y).parent = parent;
        self.replace_child(parent, x, y);
        self.at(y).right = x;
        self.at(x).parent = y;
    }
    const fn insert_fixup(&mut self, mut z: u32) {
        while self.is_red(self.parent(z)) {
            let zp = self.parent(z);
            let zpp = self.parent(zp);
            if zp == self.left(zpp) {
                let y = self.right(zpp);
                if self.is_red(y) {
                    self.set_red(zp, false);
                    self.set_red(y, false);
                    self.set_red(zpp, true);
                    z = zpp;
                } else {
                    if z == self.right(zp) {
                        z = zp;
                        self.rotate_left(z);
                    }
                    let zp = self.parent(z);
                    let zpp = self.parent(zp);
                    self.set_red(zp, false);
                    self.set_red(zpp, true);
                    self.rotate_right(zpp);
                }
            } else {
                let y = self.left(zpp);
                if self.is_red(y) {
                    self.set_red(zp, false);
                    self.set_red(y, false);
                    self.set_red(zpp, true);
                    z = zpp;
                } else {
                    if z == self.left(zp) {
                        z = zp;
                        self.rotate_right(z);
                    }
                    let zp = self.parent(z);
                    let zpp = self.parent(zp);
                    self.set_red(zp, false);
                    self.set_red(zpp, true);
                    self.rotate_left(zpp);
                }
            }
        }
        let root = self.root;
        self.set_red(root, false);
    }
    const fn transplant(&mut self, u: u32, v: u32) {
        let parent = self.parent(u);
        self.replace_child(parent, u, v);
        self.set_parent(v, parent);
    }
    const fn delete(&mut self, z: u32) {
        let mut removed_red = self.is_red(z);
        let x = if self.left(z) == NIL {
            let x = self.right(z);
            self.transplant(z, x);
            x
        } else if self.right(z) == NIL {
            let x = self.left(z);
            self.transplant(z, x);
            x
        } else {
            let mut y = self.right(z);
            while self.left(y) != NIL {
                y = self.left(y);
            }
            removed_red = self.is_red(y);
            let x = self.right(y);
            if self.parent(y) == z {
                self.set_parent(x, y);
            } else {
                self.transplant(y, x);
                let right = self.right(z);
                self.at(y).right = right;
                self.at(right).parent = y;
            }
            self.transplant(z, y);
            let left = self.left(z);
            self.at(y).left = left;
            self.at(left).parent = y;
            let red = self.is_red(z);
            self.at(y).red = red;
            x
        };
        if !removed_red {
            self.delete_fixup(x);
        }
    }
    const fn delete_fixup(&mut self, mut x: u32) {
        while x != self.root && !self.is_red(x) {
            let parent = self.parent(x);
            if x == self.left(parent) {
                let mut w = self.right(parent);
                if self.is_red(w) {
                    self.set_red(w, false);
                    self.set_red(parent, true);
                    self.rotate_left(parent);
                    w = self.right(self.parent(x));
                }
                if !self.is_red(self.left(w)) && !self.is_red(self.right(w)) {
                    self.set_red(w, true);
                    x = self.parent(x);
                } else {
                    if !self.is_red(self.right(w)) {
                        let inner = self.left(w);
                        self.set_red(inner, false);
                        self.set_red(w, true);
                        self.rotate_right(w);
                        w = self.right(self.parent(x));
                    }
                    let parent = self.parent(x);
                    let red = self.is_red(parent);
                    self.set_red(w, red);
                    self.set_red(parent, false);
                    let outer = self.right(w);
                    self.set_red(outer, false);
                    self.rotate_left(parent);
                    x = self.root;
                }
            } else {
                let mut w = self.left(parent);
                if self.is_red(w) {
                    self.set_red(w, false);
                    self.set_red(parent, true);
                    self.rotate_right(parent);
                    w = self.left(self.parent(x));
                }
                if !self.is_red(self.right(w)) && !self.is_red(self.left(w)) {
                    self.set_red(w, true);
                    x = self.parent(x);
                } else {
                    if !self.is_red(self.left(w)) {
                        let inner = self.right(w);
                        self.set_red(inner, false);
                        self.set_red(w, true);
                        self.rotate_left(w);
                        w = self.left(self.parent(x));
                    }
                    let parent = self.parent(x);
                    let red = self.is_red(parent);
                    self.set_red(w, red);
                    self.set_red(parent, false);
                    let outer = self.left(w);
                    self.set_red(outer, false);
                    self.rotate_right(parent);
                    x = self.root;
                }
            }
        }
        self.set_red(x, false);
    }
}
