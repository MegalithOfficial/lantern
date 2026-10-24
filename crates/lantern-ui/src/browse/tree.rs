//! The package tree in the sidebar. Packages open and close, a package that holds nothing but
//! one other package is shown together with it, the way file trees on the web do.

use std::collections::{BTreeMap, HashSet};

use lantern_core::render::short;

#[derive(Default)]
struct Package {
    packages: BTreeMap<String, Package>,
    /// (simple name, full name)
    classes: Vec<(String, String)>,
    total: usize,
}

#[derive(Clone, Debug)]
pub enum Target {
    Package(String),
    Class(String),
}

/// One visible line of the tree.
pub struct Row {
    pub depth: usize,
    pub label: String,
    /// Classes below a package, nothing for a class.
    pub count: Option<usize>,
    pub open: bool,
    pub target: Target,
}

#[derive(Default)]
pub struct ClassTree {
    root: Package,
    open: HashSet<String>,
}

impl ClassTree {
    pub fn new(classes: &[String]) -> Self {
        let mut root = Package::default();
        for full in classes {
            let mut parts: Vec<&str> = short(full).split('/').collect();
            let simple = parts.pop().unwrap_or(full);
            let mut package = &mut root;
            package.total += 1;
            for part in parts {
                package = package.packages.entry(part.to_owned()).or_default();
                package.total += 1;
            }
            package.classes.push((simple.to_owned(), full.clone()));
        }
        sort(&mut root);
        Self { root, open: HashSet::new() }
    }

    pub fn toggle(&mut self, path: &str) {
        if !self.open.remove(path) {
            self.open.insert(path.to_owned());
        }
    }

    /// Opens every package on the way to a class, so that it shows up in `rows`.
    pub fn reveal(&mut self, class: &str) {
        let name = short(class);
        let mut end = 0;
        for part in name.split('/') {
            if end + part.len() >= name.len() {
                break;
            }
            end += part.len();
            self.open.insert(name[..end].to_owned());
            end += 1;
        }
    }

    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        self.walk(&self.root, "", 0, &mut rows);
        rows
    }

    fn walk(&self, package: &Package, path: &str, depth: usize, rows: &mut Vec<Row>) {
        for (name, child) in &package.packages {
            // Follow a chain of packages that hold nothing else and show it as one line.
            let mut label = name.clone();
            let mut full = if path.is_empty() { name.clone() } else { format!("{path}/{name}") };
            let mut node = child;
            while node.classes.is_empty() && node.packages.len() == 1 {
                let (next, inner) = node.packages.iter().next().expect("checked the length");
                label = format!("{label}/{next}");
                full = format!("{full}/{next}");
                node = inner;
            }
            // Any package of the chain counts, `reveal` opens them one by one.
            let open = self.open.contains(&full);
            rows.push(Row { depth, label, count: Some(node.total), open, target: Target::Package(full.clone()) });
            if open {
                self.walk(node, &full, depth + 1, rows);
            }
        }
        for (simple, full) in &package.classes {
            rows.push(Row { depth, label: simple.clone(), count: None, open: false, target: Target::Class(full.clone()) });
        }
    }
}

fn sort(package: &mut Package) {
    package.classes.sort();
    package.packages.values_mut().for_each(sort);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(tree: &ClassTree) -> Vec<String> {
        tree.rows().iter().map(|r| format!("{}{}", "  ".repeat(r.depth), r.label)).collect()
    }

    #[test]
    fn folds_chains_and_reveals_classes() {
        let classes: Vec<String> = [
            "net/minecraft/house/room/lamp/oil/Lamp",
            "net/minecraft/house/room/lamp/oil/Wick",
            "net/minecraft/house/room/Shade",
            "net/minecraft/Util",
            "com/example/math/Axis",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        let mut tree = ClassTree::new(&classes);
        assert_eq!(labels(&tree), ["com/example/math", "house/room", "Util"]);

        tree.reveal("net/minecraft/house/room/lamp/oil/Lamp");
        assert_eq!(labels(&tree), ["com/example/math", "house/room", "  lamp/oil", "    Lamp", "    Wick", "  Shade", "Util"]);

        tree.toggle("house/room");
        assert_eq!(labels(&tree), ["com/example/math", "house/room", "Util"]);
    }
}
