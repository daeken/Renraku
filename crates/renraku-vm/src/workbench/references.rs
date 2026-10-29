//! Frozen facade dependencies, kept separate from live code authority.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Dependency {
    module_id: String,
    module_name: String,
    schema_hash: String,
    types: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReferenceSet {
    format: u32,
    kind: String,
    pub set_hash: String,
    pub views: Vec<ReferenceView>,
}
fn schema(view: &ReferenceView) -> Result<Schema> {
    let view = ReferenceView::from_json(&view.to_json()?)?;
    serde_json::from_str(&view.schema_json).map_err(|_| Error("invalid reference schema".into()))
}
pub(super) fn view(schema: Schema, core: String) -> Result<ReferenceView> {
    let format = if schema.dependencies.is_empty() { 1 } else { 2 };
    let schema_json = serde_json::to_string(&schema).map_err(|e| Error(e.to_string()))?;
    Ok(ReferenceView {
        format,
        schema_hash: hash(schema_json.as_bytes()),
        core_sha256: core,
        schema_json,
    })
}
fn ordering(schemas: &[Schema]) -> Result<Vec<usize>> {
    fn visit(
        index: usize,
        schemas: &[Schema],
        seen: &mut BTreeSet<usize>,
        active: &mut BTreeSet<usize>,
        order: &mut Vec<usize>,
    ) -> Result<()> {
        if seen.contains(&index) {
            return Ok(());
        }
        require(
            active.insert(index),
            "cross-module reference dependency cycle",
        )?;
        let mut dependencies = Vec::new();
        for dep in &schemas[index].dependencies {
            let target = schemas
                .iter()
                .position(|s| s.module_id == dep.module_id)
                .ok_or_else(|| Error("missing reference dependency module".into()))?;
            dependencies.push(target);
        }
        dependencies.sort_by_key(|i| (&schemas[*i].module_name, &schemas[*i].module_id));
        for target in dependencies {
            visit(target, schemas, seen, active, order)?;
        }
        active.remove(&index);
        seen.insert(index);
        order.push(index);
        Ok(())
    }
    let mut roots: Vec<_> = (0..schemas.len()).collect();
    roots.sort_by_key(|i| (&schemas[*i].module_name, &schemas[*i].module_id));
    let (mut seen, mut active, mut order) = (BTreeSet::new(), BTreeSet::new(), Vec::new());
    for index in roots {
        visit(index, schemas, &mut seen, &mut active, &mut order)?;
    }
    Ok(order)
}
/// Validate a complete frozen dependency graph without consulting live state.
pub fn check_reference_views(views: &[ReferenceView]) -> Result<()> {
    check_graph(views, views.iter().any(|view| view.format == 2))
}
fn check_graph(views: &[ReferenceView], strict_names: bool) -> Result<()> {
    require(views.len() <= 16, "reference set exceeds 16 modules")?;
    let schemas = views.iter().map(schema).collect::<Result<Vec<_>>>()?;
    let (mut ids, mut names, mut types) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for (s, v) in schemas.iter().zip(views) {
        require(
            ids.insert(&s.module_id) && names.insert(s.module_name.to_lowercase()),
            "duplicate reference module identity/name",
        )?;
        require(
            views
                .first()
                .is_none_or(|first| first.core_sha256 == v.core_sha256),
            "reference set/core mismatch",
        )?;
        for binding in s
            .bindings
            .iter()
            .filter(|b| strict_names && b.kind == "type")
        {
            require(
                types.insert(&binding.name),
                "ambiguous type name across reference modules",
            )?;
        }
    }
    ordering(&schemas)?;
    for s in &schemas {
        let mut dependencies = BTreeSet::new();
        for dep in &s.dependencies {
            require(
                dependencies.insert(&dep.module_id),
                "duplicate reference dependency",
            )?;
            let target = schemas
                .iter()
                .position(|other| other.module_id == dep.module_id)
                .ok_or_else(|| Error("missing reference dependency".into()))?;
            require(
                dep.module_name == schemas[target].module_name
                    && dep.schema_hash == views[target].schema_hash,
                "reference dependency identity/hash mismatch",
            )?;
            let bound: BTreeSet<_> = schemas[target]
                .bindings
                .iter()
                .filter(|b| b.kind == "type")
                .map(|b| b.id.as_str())
                .collect();
            require(
                !dep.types.is_empty()
                    && dep.types.len() <= 256
                    && dep.types.iter().all(|id| bound.contains(id.as_str())),
                "reference dependency type binding is missing",
            )?;
        }
    }
    Ok(())
}
/// Re-render each own-module declaration and bind every foreign ID to its facade.
pub fn validate_reference_views(views: &[ReferenceView], program: &Program) -> Result<()> {
    // Original isolated views were independently validated, including callers
    // that supplied overlapping views of one module to raw installation.
    if views.iter().any(|view| view.format == 2) {
        check_reference_views(views)?;
    }
    let schemas = views.iter().map(schema).collect::<Result<Vec<_>>>()?;
    for (given, data) in views.iter().zip(&schemas) {
        let roots = data
            .roots
            .iter()
            .map(|id| program.find_type(id))
            .collect::<Result<Vec<_>>>()?;
        let (mut actual, core, foreign) = program.reference_schema(&roots, true)?;
        for (module, types) in foreign {
            let index = schemas
                .iter()
                .position(|s| s.module_id == module.to_string())
                .ok_or_else(|| Error("missing live reference dependency".into()))?;
            actual.dependencies.push(Dependency {
                module_id: module.to_string(),
                module_name: schemas[index].module_name.clone(),
                schema_hash: views[index].schema_hash.clone(),
                types: types.iter().map(ToString::to_string).collect(),
            });
        }
        let actual = view(actual, core)?;
        require(
            actual.schema_hash == given.schema_hash && actual.core_sha256 == given.core_sha256,
            "reference view does not match the runtime schema",
        )?;
    }
    Ok(())
}
impl ReferenceSet {
    pub fn from_views(views: Vec<ReferenceView>) -> Result<Self> {
        require(!views.is_empty(), "reference set must contain a module")?;
        check_graph(&views, true)?;
        let schemas = views.iter().map(schema).collect::<Result<Vec<_>>>()?;
        let views = ordering(&schemas)?
            .into_iter()
            .map(|i| views[i].clone())
            .collect::<Vec<_>>();
        let set_hash = hash(&serde_json::to_vec(&views).map_err(|e| Error(e.to_string()))?);
        Ok(Self {
            format: 1,
            kind: "referenceSet".into(),
            set_hash,
            views,
        })
    }
    pub fn to_json(&self) -> Result<Vec<u8>> {
        let canonical = Self::from_views(self.views.clone())?;
        require(
            self.format == 1 && self.kind == "referenceSet" && self.set_hash == canonical.set_hash,
            "reference set hash/format mismatch",
        )?;
        let bytes = serde_json::to_vec(&canonical).map_err(|e| Error(e.to_string()))?;
        require(
            bytes.len() <= 4 * 1024 * 1024,
            "reference set exceeds 4 MiB",
        )?;
        Ok(bytes)
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        require(
            bytes.len() <= 4 * 1024 * 1024,
            "reference set exceeds 4 MiB",
        )?;
        let set: Self =
            serde_json::from_slice(bytes).map_err(|_| Error("invalid reference set".into()))?;
        set.to_json()?;
        Self::from_views(set.views)
    }
    /// Accept either an unchanged legacy view or a complete set envelope.
    pub fn parse_input(bytes: &[u8]) -> Result<Vec<ReferenceView>> {
        require(
            bytes.len() <= 4 * 1024 * 1024,
            "reference input exceeds 4 MiB",
        )?;
        let json: Value =
            serde_json::from_slice(bytes).map_err(|_| Error("invalid reference input".into()))?;
        if json.get("kind").is_some() {
            Ok(Self::from_json(bytes)?.views)
        } else {
            Ok(vec![ReferenceView::from_json(bytes)?])
        }
    }
    /// Keep isolated export wire bytes compatible with existing clients.
    pub fn export_json(&self) -> Result<Vec<u8>> {
        self.to_json()?;
        if self.views.len() == 1 && self.views[0].format == 1 {
            self.views[0].to_json()
        } else {
            self.to_json()
        }
    }
    pub fn validate(&self, program: &Program) -> Result<()> {
        self.to_json()?;
        validate_reference_views(&self.views, program)
    }
}
impl Program {
    pub fn reference_set(&self, roots: &[TypeId]) -> Result<ReferenceSet> {
        require(
            !roots.is_empty() && roots.len() <= 32,
            "select between 1 and 32 reference roots",
        )?;
        let mut selected: BTreeMap<AssemblyId, BTreeSet<TypeId>> = BTreeMap::new();
        for id in roots {
            let module = self
                .type_tokens
                .iter()
                .find(|((_, token), actual)| token >> 24 == 2 && *actual == id)
                .map(|((module, _), _)| *module)
                .ok_or_else(|| Error("unknown reference root".into()))?;
            selected.entry(module).or_default().insert(*id);
        }
        loop {
            require(selected.len() <= 16, "reference set exceeds 16 modules")?;
            let mut changed = false;
            for ids in selected.clone().into_values() {
                let (_, _, foreign) =
                    self.reference_schema(&ids.into_iter().collect::<Vec<_>>(), true)?;
                for (module, ids) in foreign {
                    let target = selected.entry(module).or_default();
                    for id in ids {
                        changed |= target.insert(id);
                    }
                }
            }
            if !changed {
                break;
            }
        }
        let mut schemas = Vec::new();
        let mut cores = Vec::new();
        for ids in selected.values() {
            let (mut s, core, foreign) =
                self.reference_schema(&ids.iter().copied().collect::<Vec<_>>(), true)?;
            for (module, types) in foreign {
                let image = Image::parse(&self.images[&module])?;
                s.dependencies.push(Dependency {
                    module_id: module.to_string(),
                    module_name: image.string(image.row(32, 1)?[7])?.into(),
                    schema_hash: String::new(),
                    types: types.iter().map(ToString::to_string).collect(),
                });
            }
            schemas.push(s);
            cores.push(core);
        }
        let mut views: Vec<ReferenceView> = Vec::new();
        for index in ordering(&schemas)? {
            let mut s = schemas[index].clone();
            for dep in &mut s.dependencies {
                dep.schema_hash = views
                    .iter()
                    .find(|v| schema(v).is_ok_and(|s| s.module_id == dep.module_id))
                    .unwrap()
                    .schema_hash
                    .clone();
            }
            views.push(view(s, cores[index].clone())?);
        }
        ReferenceSet::from_views(views)
    }
}
