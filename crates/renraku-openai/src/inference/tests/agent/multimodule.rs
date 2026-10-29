//! Scripted wire acceptance for installed CIL libraries and dependency sets.
use super::*;
use renraku_vm::{
    artifact::install_once,
    compiler::{CompilationRequest, CompilerToolchain},
    identity::{FieldId, ObjectId, TypeId},
    value::Value as GuestValue,
    web::{WebHost, WebRequest, WebServer, inspect_publications, publish_handler},
};

const GOAL: &str = "Discover the saved grouped website map, inspect its code/contracts, correct a caller from compiler diagnostics, update red once, and publish/preview a read-only page using the same map and decimal library.";
const CALLER: &str = "public static class HttpGroupsCaller { public static string Run() { var previous=GroupedWebsite.Counts.Get(\"red\"); if (!previous.Success || previous.Value!=\"2\") return \"unexpected replay\"; return GroupedWebsite.Counts.Set(\"red\",\"3\").Value; } }";
const BAD_CALLER: &str = "public static class HttpGroupsCaller { public static string Run() { return GroupedWebsite.Counts.Set(\"red\",7).Value; } }";
const PAGE: &str = "public static class HttpGroupsPage { public static string Handle(string id,string method,string target,string body) { var result=GroupedWebsite.Counts.Get(\"red\"); string value=result.Success?result.Value:\"missing\"; return string.Concat(string.Concat(\"<h1>Shared map verified</h1><p>red=\",value),string.Concat(\"; total=\",string.Concat(Renraku.Text.IntegerText.FormatInt32(Renraku.Guest.Website.Visits),\"</p>\"))); } }";
struct Facts {
    map: ObjectId,
    map_type: TypeId,
    counts: FieldId,
    owner: TypeId,
    node: ObjectId,
}
fn prepare(
    host: &AgentHost,
    profile: Uuid,
) -> (Facts, Vec<(WebRequest, renraku_vm::web::WebReply)>) {
    let program = common::program();
    let mut vm = Vm::new(&program);
    let mut store = CheckpointStore::open(&host.universe).unwrap();
    vm.start(
        program.entry("Renraku.Guest.Website::Initialize").unwrap(),
        vec![],
    )
    .unwrap();
    vm.run_checkpointed(&mut store, 1000, 100).unwrap();
    let dotnet = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|p| p.join("dotnet"))
        .find(|p| p.is_file())
        .unwrap();
    let toolchain = CompilerToolchain::open(dotnet, compiler()).unwrap();
    let mut compiler_store = CompilationStore::open(&host.compiler_ledger).unwrap();
    for (name, file, source) in [
        (
            "Renraku.Library",
            "IntegerText.cs",
            include_bytes!("../../../../../../guest/IntegerText.cs").as_slice(),
        ),
        (
            "Renraku.Collections",
            "StringMap.cs",
            include_bytes!("../../../../../../guest/StringMap.cs").as_slice(),
        ),
        (
            "Generated.GroupedWebsite",
            "GroupedWebsite.cs",
            include_bytes!("../../../../../../guest/examples/GroupedWebsite.cs").as_slice(),
        ),
    ] {
        let saved = store.load().unwrap();
        let program = &saved.snapshot.program;
        let views = if file == "GroupedWebsite.cs" {
            program
                .reference_set(&[
                    program.find_type("Renraku.Guest.Website").unwrap(),
                    program.find_type("Renraku.Text.IntegerText").unwrap(),
                    program.find_type("Renraku.Collections.StringMap").unwrap(),
                ])
                .unwrap()
                .views
        } else {
            vec![]
        };
        let input = CompilationRequest::capture(
            program,
            vm.universe_id(),
            &toolchain,
            name.into(),
            vec![(file.into(), source.to_vec())],
            views,
        )
        .unwrap();
        compiler_store.prepare(name, &input).unwrap();
        let built = compiler_store
            .drive(vm.universe_id(), name, &toolchain)
            .unwrap();
        install_once(
            &mut store,
            &format!("setup-{name}"),
            built.artifact.as_ref().unwrap(),
        )
        .unwrap();
    }
    publish_handler(&mut store, "setup-groups", "GroupedWebsite::Handle").unwrap();
    let edge = WebHost::open_published(&host.universe).unwrap();
    let mut baseline = Vec::new();
    for id in ["before-agent-one", "before-agent-two"] {
        let request = WebRequest {
            id: id.into(),
            method: "POST".into(),
            target: "/visit".into(),
            body: "red".into(),
        };
        let response = edge.process(&request, 10000).unwrap();
        baseline.push((request, response));
    }
    let saved = store.load().unwrap();
    let program = &saved.snapshot.program;
    let mut vm = saved.snapshot.restore().unwrap();
    let owner = program.find_type("GroupedWebsite").unwrap();
    let counts = program
        .fields()
        .find(|f| f.owner == owner && f.name == "Counts")
        .unwrap()
        .id;
    let map = vm.static_fields()[&counts].referenced_object().unwrap();
    let map_type = vm.heap().object(map).unwrap().type_id();
    let head = program
        .fields()
        .find(|f| f.owner == map_type && f.name == "head")
        .unwrap()
        .id;
    let node = vm.heap().object(map).unwrap().fields()[&head]
        .referenced_object()
        .unwrap();
    AgentHost::initialize(
        &mut vm,
        &mut store,
        &OpenAIProvider::capability_for(profile),
        "chosen-model",
        GOAL,
    )
    .unwrap();
    (
        Facts {
            map,
            map_type,
            counts,
            owner,
            node,
        },
        baseline,
    )
}
fn selected(request: &Observed, map: String, node: String) -> Value {
    assert_eq!(request.path, "/responses");
    assert_eq!(request.bearer, "Bearer coding-token");
    let body = &request.body;
    assert_eq!(body.as_object().unwrap().len(), 4);
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
    assert_eq!(body["model"], "chosen-model");
    assert_eq!(body["input"][1]["content"], GOAL);
    let count = assistant_count(body);
    let last = if count > 0 {
        last_result(body)
    } else {
        Value::Null
    };
    match count {
        0 => object!({"op":"search","query":"GroupedWebsite","limit":20}),
        1 => object!({"op":"inspect_static","field":"GroupedWebsite::Counts"}),
        2 => {
            assert_eq!(last["output"]["field"]["value"]["id"], map);
            object!({"op":"inspect_object","id":map})
        }
        3 => {
            assert_eq!(last["output"]["objectId"], map);
            object!({"op":"describe","type":"GroupedWebsite"})
        }
        4 => {
            assert_eq!(last["ok"], true);
            assert_eq!(last["dependencies"].as_array().unwrap().len(), 1);
            assert_eq!(last["dependencies"][0]["moduleName"], "Renraku.Collections");
            object!({"op":"read_code","method":"Renraku.Collections.StringMap::Set"})
        }
        5 => {
            assert_eq!(last["truncated"], true);
            object!({"op":"read_code","method":"Renraku.Collections.StringMap::Set","sourceOnly":true})
        }
        6 => {
            assert_eq!(last["output"]["sourceAvailable"], true, "{last}");
            assert_eq!(last["output"]["representation"], "source_provenance");
            assert!(last["output"].get("instructions").is_none());
            assert!(
                last["output"]["sources"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("Equal content is a no-op")
            );
            object!({"op":"read_code","method":"GroupedWebsite::Handle","sourceOnly":true})
        }
        7 => {
            assert_eq!(last["output"]["sourceAvailable"], true);
            object!({"op":"compile","assembly":"Generated.HttpGroupsCaller","source":BAD_CALLER,"referenceTypes":["GroupedWebsite"]})
        }
        8 => {
            assert_eq!(last["output"]["state"], "rejected");
            assert!(
                last["output"]["report"]["diagnostics"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|d| d["code"] == "CS1503")
            );
            object!({"op":"compile","assembly":"Generated.HttpGroupsCaller","source":CALLER,"referenceTypes":["GroupedWebsite"]})
        }
        9 => {
            assert_eq!(last["output"]["state"], "compiled");
            object!({"op":"install","compilationTurn":9})
        }
        10 => object!({"op":"invoke","method":"HttpGroupsCaller::Run","arguments":[]}),
        11 => {
            assert_eq!(last["ok"], true);
            object!({"op":"inspect_object","id":map})
        }
        12 => {
            assert_eq!(last["output"]["objectId"], map);
            object!({"op":"inspect_object","id":node})
        }
        13 => {
            let field = last["output"]["fields"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| f["name"] == "Value")
                .unwrap();
            assert_eq!(field["value"]["stringPreview"]["text"], "3");
            object!({"op":"compile","assembly":"Generated.HttpGroupsPage","source":PAGE,"referenceTypes":["GroupedWebsite","Renraku.Text.IntegerText","Renraku.Guest.Website"]})
        }
        14 => object!({"op":"install","compilationTurn":14}),
        15 => object!({"op":"publish_web","method":"HttpGroupsPage::Handle"}),
        16 => object!({"op":"preview_web","method":"GET","target":"/page"}),
        17 => {
            assert!(
                last["preview"]["body"]
                    .as_str()
                    .unwrap()
                    .contains("red=3; total=2")
            );
            object!({"op":"finish","message":"same saved map observed and page verified"})
        }
        _ => panic!("unexpected multi-module HTTP turn"),
    }
}
fn collect(host: &AgentHost) {
    let mut store = CheckpointStore::open(&host.universe).unwrap();
    let saved = store.load().unwrap();
    let mut vm = saved.snapshot.restore().unwrap();
    vm.collect_garbage().unwrap();
    store.save(&vm).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn http_multi_module_map_workflow_corrects_diagnostics_and_reconciles_every_mutating_tool() {
    let directory = Directory::new();
    let id = profile(&directory.0.join("auth"), "coding", "coding-token");
    let mut host = host(&directory.0, compiler());
    host.allow_web_publication = true;
    let (facts, baseline) = prepare(&host, id);
    let map = facts.map.to_string();
    let node = facts.node.to_string();
    let server = Server::with_responder(
        move |request| {
            FixtureReply::completed(&selected(request, map.clone(), node.clone()).to_string())
        },
        false,
    );
    for target in [6, 7, 8, 9, 10, 11, 14, 15, 16] {
        let mut provider = async_adapter(&directory.0, id, &server.base).await;
        let error = host
            .drive_with_observer(&mut provider, budgets(30), |stage| {
                if stage == AgentStage::ToolCompleted(target) {
                    Err(VmError("lost multi-module result".into()))
                } else {
                    Ok(())
                }
            })
            .await
            .unwrap_err();
        assert!(error.0.contains("lost multi-module result"));
        collect(&host);
        if target == 16 {
            let mut store = CheckpointStore::open(&host.universe).unwrap();
            publish_handler(&mut store, "operator-newer", "HttpGroupsPage::Handle").unwrap();
            host.allow_web_publication = false;
        }
        let mut offline_host = host.clone();
        offline_host.dotnet = directory.0.join("missing-dotnet");
        offline_host.compiler_driver = directory.0.join("missing-compiler.dll");
        let mut offline = async_adapter(&directory.0, id, "http://127.0.0.1:1").await;
        let before = server.posts();
        let report = offline_host.drive(&mut offline, budgets(0)).await.unwrap();
        assert_eq!(report.provider_operations, 0);
        assert_eq!(server.posts(), before);
        if target == 16 {
            let mut store = CheckpointStore::open(&host.universe).unwrap();
            assert_eq!(
                inspect_publications(&mut store).unwrap()["current"]["id"],
                "operator-newer"
            );
        }
    }
    let mut provider = async_adapter(&directory.0, id, &server.base).await;
    let report = host.drive(&mut provider, budgets(30)).await.unwrap();
    assert!(matches!(report.stop, AgentStop::Finished(_)));
    assert_eq!(server.posts(), 18);
    let mut store = CheckpointStore::open(&host.universe).unwrap();
    let saved = store.load().unwrap();
    let program = &saved.snapshot.program;
    let vm = saved.snapshot.restore().unwrap();
    let status = vm.agent_status().unwrap();
    assert_eq!(program.find_type("GroupedWebsite").unwrap(), facts.owner);
    assert_eq!(
        vm.static_fields()[&facts.counts],
        GuestValue::Reference(facts.map)
    );
    assert_eq!(
        vm.heap()
            .objects()
            .filter(|o| o.type_id() == facts.map_type)
            .count(),
        1
    );
    assert_eq!(status["turn"], 18);
    assert_http_history(&server, &status);
    assert_eq!(receipt_counts(&directory.0), (18, 18));
    let edge = WebHost::open_published(&host.universe).unwrap();
    for (request, original) in baseline {
        let replay = edge.process(&request, 10000).unwrap();
        assert_eq!(replay.body, original.body);
        assert_eq!(replay.execution, original.execution);
        assert_eq!(replay.instructions, 0);
    }
    fn exchange(edge: &WebHost) -> Vec<u8> {
        use std::io::{Read, Write};
        let server = WebServer::bind(edge.clone(), "127.0.0.1:0".parse().unwrap(), 10000).unwrap();
        let address = server.address().unwrap();
        let worker = std::thread::spawn(move || server.serve_one());
        let mut socket = std::net::TcpStream::connect(address).unwrap();
        socket.write_all(b"GET /page HTTP/1.1\r\nHost: localhost\r\nX-Renraku-Request-Id: multi-native\r\nContent-Length: 0\r\n\r\n").unwrap();
        socket.shutdown(std::net::Shutdown::Write).unwrap();
        let mut out = Vec::new();
        socket.read_to_end(&mut out).unwrap();
        worker.join().unwrap().unwrap();
        out
    }
    let first = exchange(&edge);
    assert!(
        std::str::from_utf8(&first)
            .unwrap()
            .contains("red=3; total=2")
    );
    assert_eq!(exchange(&edge), first);
    let mut offline = async_adapter(&directory.0, id, "http://127.0.0.1:1").await;
    host.dotnet = directory.0.join("missing-dotnet");
    host.compiler_driver = directory.0.join("missing-compiler.dll");
    let report = host.drive(&mut offline, budgets(0)).await.unwrap();
    assert!(matches!(report.stop, AgentStop::Finished(_)));
    assert_eq!(report.instructions, 0);
    assert_eq!(report.tool_actions, 0);
    assert_eq!(report.provider_operations, 0);
}
