//! Index behaviour on a synthetic root with hand written code.

use std::collections::HashMap;
use std::sync::Mutex;

use jadx_core::model::node::{MethodId, Root};
use jadx_index::{
    build_call_graph, component_of, dispatch_targets, Access, BuildOptions, CallTarget, CodeSource,
    Component, Direction, GraphLimits, Insn, MatchMode, NameIndex, NameKind, NameQuery, Operand,
    PackageScope, Query, SiteKind, StoreConfig, UsageIndex,
};
use jadx_input_api::{ClassData, FieldData, FieldRefData, MethodData, MethodRefData, Opcode};

fn method(name: &str, code: Option<u32>) -> MethodData {
    MethodData {
        name: name.into(),
        arg_types: vec![],
        return_type: "V".into(),
        access_flags: 0x1,
        signature: None,
        code_offset: code,
        annotation_refs: vec![],
        attrs: Default::default(),
    }
}

fn class(desc: &str, sup: &str, methods: Vec<MethodData>, fields: Vec<&str>) -> ClassData {
    ClassData {
        type_desc: desc.into(),
        access_flags: 0x1,
        super_type: Some(sup.into()),
        interfaces: vec![],
        signature: None,
        inner_class: None,
        annotation_refs: vec![],
        fields: fields
            .into_iter()
            .map(|n| FieldData {
                name: n.into(),
                ty: "I".into(),
                access_flags: 0x1,
                signature: None,
                const_value: None,
                annotation_refs: vec![],
                attrs: Default::default(),
            })
            .collect(),
        methods,
        attrs: Default::default(),
    }
}

fn mref(cls: &str, name: &str) -> MethodRefData {
    MethodRefData {
        parent_class: cls.into(),
        name: name.into(),
        arg_types: vec![],
        return_type: "V".into(),
    }
}

fn insn(offset: u32, opcode: Opcode, literal: i64, operand: Operand) -> Insn {
    Insn {
        offset,
        opcode,
        literal,
        operand,
    }
}

/// Code keyed by `(class descriptor, code offset)`; mutable so tests can change it after the
/// build.
struct FakeSource {
    descs: Vec<String>,
    code: Mutex<HashMap<(String, u32), Vec<Insn>>>,
}

impl CodeSource for FakeSource {
    fn visit_method(
        &self,
        class_idx: usize,
        code_offset: u32,
        f: &mut dyn FnMut(&Insn),
    ) -> jadx_index::Result<()> {
        let code = self.code.lock().unwrap();
        let key = (self.descs[class_idx].clone(), code_offset);
        match code.get(&key) {
            Some(insns) => {
                for i in insns {
                    if SiteKind::of(i.opcode).is_some() {
                        f(i);
                    }
                }
                Ok(())
            }
            None => Err(jadx_index::IndexError::Source("no code".into())),
        }
    }
}

struct Fixture {
    root: Root,
    source: FakeSource,
}

fn fixture() -> Fixture {
    let classes = vec![
        class(
            "Lp/Base;",
            "Landroid/app/Activity;",
            vec![method("run", Some(1)), method("<init>", None)],
            vec!["x", "y"],
        ),
        class(
            "Lp/Sub;",
            "Lp/Base;",
            vec![
                method("go", Some(2)),
                method("other", Some(3)),
                method("bad", Some(4)),
            ],
            vec![],
        ),
        class(
            "Lp/Util;",
            "Ljava/lang/Object;",
            vec![method("helper", Some(5))],
            vec![],
        ),
    ];
    let root = Root::build(classes, true).unwrap();
    let descs: Vec<String> = root
        .classes
        .iter()
        .map(|c| format!("L{};", c.info.raw_name().replace('.', "/")))
        .collect();
    let field = |c: &str, n: &str| {
        Operand::Field(FieldRefData {
            parent_class: c.into(),
            name: n.into(),
            ty: "I".into(),
        })
    };
    let mut code: HashMap<(String, u32), Vec<Insn>> = HashMap::new();
    // Base.run: reads x, calls Util.helper
    code.insert(
        ("Lp/Base;".into(), 1),
        vec![
            insn(0, Opcode::Iget, 0, field("Lp/Base;", "x")),
            insn(
                2,
                Opcode::InvokeStatic,
                0,
                Operand::Method(mref("Lp/Util;", "helper")),
            ),
        ],
    );
    // Sub.go: calls run through the sub class, writes x through the sub class, literals, type uses
    code.insert(
        ("Lp/Sub;".into(), 2),
        vec![
            insn(
                0,
                Opcode::InvokeVirtual,
                0,
                Operand::Method(mref("Lp/Sub;", "run")),
            ),
            insn(3, Opcode::Iput, 0, field("Lp/Sub;", "x")),
            insn(5, Opcode::ConstString, 0, Operand::Str("hello".into())),
            insn(7, Opcode::Const, 42, Operand::None),
            insn(9, Opcode::NewInstance, 0, Operand::Type("Lp/Base;".into())),
            insn(11, Opcode::AddIntLit, 7, Operand::None),
        ],
    );
    // Sub.other: external call, reads y, other literals
    code.insert(
        ("Lp/Sub;".into(), 3),
        vec![
            insn(
                0,
                Opcode::InvokeStatic,
                0,
                Operand::Method(mref("Landroid/util/Log;", "d")),
            ),
            insn(2, Opcode::Sget, 0, field("Lp/Base;", "y")),
            insn(
                4,
                Opcode::ConstString,
                0,
                Operand::Str("hello world".into()),
            ),
            insn(6, Opcode::Const, 42, Operand::None),
            insn(8, Opcode::Const, -1, Operand::None),
            insn(
                10,
                Opcode::InvokeVirtual,
                0,
                Operand::Method(mref("Lp/Sub;", "go")),
            ),
            insn(12, Opcode::CheckCast, 0, Operand::Type("Lp/Base;".into())),
        ],
    );
    // Util.helper calls Sub.go (a cycle through Base.run)
    code.insert(
        ("Lp/Util;".into(), 5),
        vec![insn(
            0,
            Opcode::InvokeVirtual,
            0,
            Operand::Method(mref("Lp/Sub;", "go")),
        )],
    );
    // method 4 is not present: undecodable
    Fixture {
        root,
        source: FakeSource {
            descs,
            code: Mutex::new(code),
        },
    }
}

fn mid(root: &Root, cls: &str, name: &str) -> MethodId {
    let ci = root.class_index(cls).unwrap();
    let mi = root.classes[ci]
        .methods
        .iter()
        .position(|m| m.info.name == name)
        .unwrap();
    (ci, mi)
}

fn build<'a>(f: &'a Fixture, opts: &BuildOptions) -> UsageIndex<'a> {
    UsageIndex::build(&f.root, &f.source, opts).unwrap()
}

fn names_of(root: &Root, sites: &jadx_index::Sites) -> Vec<String> {
    sites
        .sites
        .iter()
        .map(|s| {
            format!(
                "{}.{}@{}:{}",
                root.classes[s.class].info.raw_name(),
                root.classes[s.class].methods[s.method].info.name,
                s.offset,
                s.kind.name()
            )
        })
        .collect()
}

#[test]
fn callers_resolve_through_subclass_references() {
    let f = fixture();
    let idx = build(&f, &BuildOptions::default());
    assert_eq!(idx.stats().methods_scanned, 5);
    assert_eq!(idx.stats().methods_undecodable, 1);
    let q = Query::default();
    // Sub.go calls `Sub.run`, declared in Base
    let callers = idx.callers_of(mid(&f.root, "p.Base", "run"), &q).unwrap();
    assert_eq!(names_of(&f.root, &callers), ["p.Sub.go@0:invoke-virtual"]);
    let callers = idx.callers_of(mid(&f.root, "p.Sub", "go"), &q).unwrap();
    assert_eq!(
        names_of(&f.root, &callers),
        [
            "p.Sub.other@10:invoke-virtual",
            "p.Util.helper@0:invoke-virtual"
        ]
    );
    // external method
    let log = idx
        .callers_of_ref(&mref("Landroid/util/Log;", "d"), &q)
        .unwrap();
    assert_eq!(names_of(&f.root, &log), ["p.Sub.other@0:invoke-static"]);
    // nothing calls Util.helper except Base.run
    let helper = idx
        .callers_of(mid(&f.root, "p.Util", "helper"), &q)
        .unwrap();
    assert_eq!(names_of(&f.root, &helper), ["p.Base.run@2:invoke-static"]);
    assert!(idx
        .callers_of(mid(&f.root, "p.Sub", "bad"), &q)
        .unwrap()
        .sites
        .is_empty());
}

#[test]
fn field_readers_and_writers() {
    let f = fixture();
    let idx = build(&f, &BuildOptions::default());
    let base = f.root.class_index("p.Base").unwrap();
    let x = f.root.classes[base]
        .fields
        .iter()
        .position(|f| f.info.name == "x")
        .unwrap();
    let q = Query::default();
    let all = idx.field_sites((base, x), Access::Any, &q).unwrap();
    assert_eq!(
        names_of(&f.root, &all),
        ["p.Base.run@0:field-read", "p.Sub.go@3:field-write"]
    );
    let w = idx.field_sites((base, x), Access::Write, &q).unwrap();
    assert_eq!(names_of(&f.root, &w), ["p.Sub.go@3:field-write"]);
    let r = idx.field_sites((base, x), Access::Read, &q).unwrap();
    assert_eq!(names_of(&f.root, &r), ["p.Base.run@0:field-read"]);
    // a field reference naming the subclass resolves to the declaration
    let by_ref = idx
        .field_sites_of_ref(
            &FieldRefData {
                parent_class: "Lp/Sub;".into(),
                name: "y".into(),
                ty: "I".into(),
            },
            Access::Read,
            &q,
        )
        .unwrap();
    assert_eq!(names_of(&f.root, &by_ref), ["p.Sub.other@2:field-read"]);
}

#[test]
fn literals_and_type_uses() {
    let f = fixture();
    let idx = build(&f, &BuildOptions::default());
    let q = Query::default();
    assert_eq!(
        names_of(&f.root, &idx.string_sites("hello", &q).unwrap()),
        ["p.Sub.go@5:string"]
    );
    assert!(idx.string_sites("hell", &q).unwrap().sites.is_empty());
    assert_eq!(
        names_of(&f.root, &idx.int_sites(42, &q).unwrap()),
        ["p.Sub.go@7:int-literal", "p.Sub.other@6:int-literal"]
    );
    assert_eq!(idx.int_sites(-1, &q).unwrap().sites.len(), 1);
    assert_eq!(idx.int_sites(7, &q).unwrap().sites.len(), 1); // add-int/lit
    assert_eq!(
        names_of(&f.root, &idx.type_sites("Lp/Base;", &q).unwrap()),
        ["p.Sub.go@9:new-instance", "p.Sub.other@12:check-cast"]
    );
    // substring and range scans
    let found = idx
        .search_strings("HELLO", true, &PackageScope::all(), 10, None)
        .unwrap();
    assert_eq!(found.len(), 2);
    assert!(idx
        .search_strings("HELLO", false, &PackageScope::all(), 10, None)
        .unwrap()
        .is_empty());
    let ints = idx
        .search_int_range(0, 100, &PackageScope::all(), 10, None)
        .unwrap();
    assert_eq!(ints.len(), 3);
    assert_eq!(
        idx.search_int_range(0, 100, &PackageScope::all(), 2, None)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn limits_scope_and_verification() {
    let f = fixture();
    let idx = build(&f, &BuildOptions::default());
    let limited = Query {
        limit: 1,
        ..Query::default()
    };
    let r = idx.int_sites(42, &limited).unwrap();
    assert_eq!((r.sites.len(), r.truncated), (1, true));
    let scoped = Query {
        scope: PackageScope::parse(&["p.Util"]).unwrap(),
        ..Query::default()
    };
    let r = idx
        .callers_of(mid(&f.root, "p.Sub", "go"), &scoped)
        .unwrap();
    assert_eq!(names_of(&f.root, &r), ["p.Util.helper@0:invoke-virtual"]);

    // the code changes after the build: verified queries drop the stale hit
    f.source
        .code
        .lock()
        .unwrap()
        .insert(("Lp/Util;".into(), 5), vec![]);
    let q = Query::default();
    let target = mid(&f.root, "p.Sub", "go");
    assert_eq!(idx.callers_of(target, &q).unwrap().sites.len(), 1);
    let unverified = Query {
        verify: false,
        ..Query::default()
    };
    assert_eq!(idx.callers_of(target, &unverified).unwrap().sites.len(), 2);
}

#[test]
fn scope_restricts_the_build() {
    let f = fixture();
    let opts = BuildOptions {
        scope: PackageScope::parse(&["p.Sub"]).unwrap(),
        ..BuildOptions::default()
    };
    let idx = build(&f, &opts);
    assert_eq!(idx.stats().methods_scanned, 3);
    let callers = idx
        .callers_of(mid(&f.root, "p.Sub", "go"), &Query::default())
        .unwrap();
    assert_eq!(callers.sites.len(), 1); // Util.helper was not scanned
}

#[test]
fn disk_backed_index_gives_the_same_answers() {
    let f = fixture();
    let mem = build(&f, &BuildOptions::default());
    let opts = BuildOptions {
        store: StoreConfig::disk(std::env::temp_dir(), 40),
        threads: 2,
        ..BuildOptions::default()
    };
    let disk = build(&f, &opts);
    assert!(disk.stats().on_disk && !mem.stats().on_disk);
    assert_eq!(disk.stats().sites, mem.stats().sites);
    let q = Query::default();
    for target in [("p.Sub", "go"), ("p.Base", "run"), ("p.Util", "helper")] {
        let id = mid(&f.root, target.0, target.1);
        assert_eq!(
            disk.callers_of(id, &q).unwrap(),
            mem.callers_of(id, &q).unwrap()
        );
    }
    assert_eq!(
        disk.int_sites(42, &q).unwrap(),
        mem.int_sites(42, &q).unwrap()
    );
    // a memory-only index with a tiny budget fails instead of growing
    let tiny = BuildOptions {
        store: StoreConfig::memory(40),
        threads: 1,
        ..BuildOptions::default()
    };
    assert!(UsageIndex::build(&f.root, &f.source, &tiny).is_err());
}

#[test]
fn cancelled_build_fails() {
    let f = fixture();
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let opts = BuildOptions {
        cancel: Some(cancel),
        ..BuildOptions::default()
    };
    assert!(matches!(
        UsageIndex::build(&f.root, &f.source, &opts),
        Err(jadx_index::IndexError::Cancelled)
    ));
}

#[test]
fn callees_and_call_graph() {
    let f = fixture();
    let idx = build(&f, &BuildOptions::default());
    let go = mid(&f.root, "p.Sub", "go");
    let callees = idx.callees_of(go).unwrap();
    assert_eq!(callees.len(), 1);
    assert_eq!(
        callees[0].target,
        CallTarget::Method(mid(&f.root, "p.Base", "run"))
    );
    let other = idx.callees_of(mid(&f.root, "p.Sub", "other")).unwrap();
    assert!(matches!(&other[0].target, CallTarget::External(r) if r.name == "d"));

    let limits = GraphLimits {
        depth: 2,
        ..GraphLimits::default()
    };
    let g = build_call_graph(&idx, go, Direction::Callees, &limits, None).unwrap();
    // go -> run -> helper
    let texts: Vec<&str> = g.nodes.iter().map(|n| n.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "p.Sub.go():void",
            "p.Base.run():void",
            "p.Util.helper():void"
        ]
    );
    assert_eq!(g.edges.len(), 2);
    assert!(!g.truncated);
    assert_eq!(g.nodes[2].depth, 2);

    let g = build_call_graph(&idx, go, Direction::Callers, &limits, None).unwrap();
    // other and helper call go; helper's caller Base.run is at depth 2, and it calls go again
    let texts: Vec<&str> = g.nodes.iter().map(|n| n.text.as_str()).collect();
    assert!(texts.contains(&"p.Sub.other():void"));
    assert!(texts.contains(&"p.Util.helper():void"));
    assert!(texts.contains(&"p.Base.run():void"));
    assert!(g
        .edges
        .iter()
        .all(|e| g.nodes[e.from].depth > g.nodes[e.to].depth));

    let both = build_call_graph(&idx, go, Direction::Both, &limits, None).unwrap();
    assert!(both.edges.len() > g.edges.len());

    let capped = GraphLimits {
        max_nodes: 2,
        ..limits.clone()
    };
    let g = build_call_graph(&idx, go, Direction::Both, &capped, None).unwrap();
    assert!(g.truncated && g.nodes.len() == 2);
    assert!(build_call_graph(
        &idx,
        go,
        Direction::Callees,
        &GraphLimits { depth: 0, ..limits },
        None
    )
    .is_err());
}

#[test]
fn components_and_dispatch_targets() {
    let f = fixture();
    let sub = f.root.class_index("p.Sub").unwrap();
    assert_eq!(component_of(&f.root, sub), Some(Component::Activity));
    assert_eq!(
        component_of(&f.root, f.root.class_index("p.Util").unwrap()),
        None
    );
    // Sub.go does not override anything; a method with the name of a Base method would
    let (targets, truncated) = dispatch_targets(&f.root, mid(&f.root, "p.Sub", "go"));
    assert!(targets.is_empty() && !truncated);
    // constructors never dispatch
    assert!(dispatch_targets(&f.root, mid(&f.root, "p.Base", "<init>"))
        .0
        .is_empty());
}

#[test]
fn name_search_modes() {
    let f = fixture();
    let names = NameIndex::build(&f.root, 1000).unwrap();
    assert_eq!(names.len(), 3 + 6 + 2);
    assert!(NameIndex::build(&f.root, 3).is_err());
    let hits = |q: NameQuery| -> Vec<String> {
        names
            .search(&q)
            .hits
            .into_iter()
            .map(|h| h.display)
            .collect()
    };
    assert_eq!(hits(NameQuery::new("sub", MatchMode::Exact)), ["p.Sub"]);
    assert_eq!(
        hits(NameQuery::new("RU", MatchMode::Prefix)),
        ["p.Base.run():void"]
    );
    let mut q = NameQuery::new("RU", MatchMode::Prefix);
    q.case_sensitive = true;
    assert!(hits(q).is_empty());
    let mut q = NameQuery::new("*e*", MatchMode::Glob);
    q.kinds = vec![NameKind::Class];
    assert_eq!(hits(q), ["p.Base"]);
    assert_eq!(hits(NameQuery::new("x", MatchMode::Word)), ["p.Base.x:int"]);
    let mut q = NameQuery::new("", MatchMode::Substring);
    q.scope = PackageScope::parse(&["p.Util"]).unwrap();
    assert_eq!(hits(q).len(), 2);
    let mut q = NameQuery::new("", MatchMode::Substring);
    q.limit = 3;
    let r = names.search(&q);
    assert_eq!((r.hits.len(), r.truncated), (3, true));
    // camel case matching is always case sensitive
    assert_eq!(hits(NameQuery::new("B", MatchMode::CamelCase)), ["p.Base"]);
    assert!(hits(NameQuery::new("Q", MatchMode::CamelCase)).is_empty());
}

#[test]
fn root_usage_lists_are_plain_views() {
    let f = fixture();
    // the synthetic root did not run the usage pass: the lists are empty but readable
    let go = mid(&f.root, "p.Sub", "go");
    assert!(jadx_index::root_usage::method_users(&f.root, go).is_empty());
    assert!(jadx_index::root_usage::method_callees(&f.root, go).is_empty());
    assert!(jadx_index::root_usage::method_external_callees(&f.root, go).is_empty());
    assert!(jadx_index::root_usage::field_users(&f.root, (0, 0)).is_empty());
    let (c, m) = jadx_index::root_usage::class_users(&f.root, 0);
    assert!(c.is_empty() && m.is_empty());
}
