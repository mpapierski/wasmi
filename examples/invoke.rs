extern crate parity_wasm;
extern crate wasmi;

use std::{
    collections::BTreeMap,
    env::args, time::{Duration}, fs::{self, File}, io::Write,
};


use parity_wasm::elements::{External, FunctionType, Internal, Module, Type, ValueType};
use quanta::{Instant, Clock};
use wasmi::{
    profiler::{Profiler, Instruction},
    ImportsBuilder, ModuleInstance, NopExternals, RuntimeValue,
};

#[derive(Debug)]
struct TracingProfiler {
    clock: Clock,
    trace: Vec<(Instruction, Instant)>,
}

impl TracingProfiler {
    fn new() -> Self {
        Self { trace: Vec::new(), clock: Clock::new() }
    }
}


impl Profiler for TracingProfiler {
    type Timestamp = Instant;

    fn trace<'a>(&mut self, instruction: Instruction, clock: Self::Timestamp) {
        self.trace.push((instruction, clock));
    }


    fn sample_time(&mut self) -> Self::Timestamp {
        Instant::now()
    }
}

fn main() {
    let args: Vec<_> = args().collect();
    if args.len() < 3 {
        println!("Usage: {} <wasm file> <exported func> [<arg>...]", args[0]);
        return;
    }
    let func_name = &args[2];
    let (_, program_args) = args.split_at(3);

    let module = load_module(&args[1]);

    // Extracts call arguments from command-line arguments
    let args = {
        // Export section has an entry with a func_name with an index inside a module
        let export_section = module.export_section().expect("No export section found");
        // It's a section with function declarations (which are references to the type section entries)
        let function_section = module
            .function_section()
            .expect("No function section found");
        // Type section stores function types which are referenced by function_section entries
        let type_section = module.type_section().expect("No type section found");

        // Given function name used to find export section entry which contains
        // an `internal` field which points to the index in the function index space
        let found_entry = export_section
            .entries()
            .iter()
            .find(|entry| func_name == entry.field())
            .unwrap_or_else(|| panic!("No export with name {} found", func_name));

        // Function index in the function index space (internally-defined + imported)
        let function_index: usize = match found_entry.internal() {
            Internal::Function(index) => *index as usize,
            _ => panic!("Founded export is not a function"),
        };

        // We need to count import section entries (functions only!) to subtract it from function_index
        // and obtain the index within the function section
        let import_section_len: usize = match module.import_section() {
            Some(import) => import
                .entries()
                .iter()
                .filter(|entry| matches!(entry.external(), External::Function(_)))
                .count(),
            None => 0,
        };

        // Calculates a function index within module's function section
        let function_index_in_section = function_index - import_section_len;

        // Getting a type reference from a function section entry
        let func_type_ref: usize =
            function_section.entries()[function_index_in_section].type_ref() as usize;

        // Use the reference to get an actual function type
        #[allow(clippy::infallible_destructuring_match)]
        let function_type: &FunctionType = match &type_section.types()[func_type_ref] {
            Type::Function(ref func_type) => func_type,
        };

        // Parses arguments and constructs runtime values in correspondence of their types
        function_type
            .params()
            .iter()
            .enumerate()
            .map(|(i, value)| match value {
                ValueType::I32 => RuntimeValue::I32(
                    program_args[i]
                        .parse::<i32>()
                        .unwrap_or_else(|_| panic!("Can't parse arg #{} as i32", program_args[i])),
                ),
                ValueType::I64 => RuntimeValue::I64(
                    program_args[i]
                        .parse::<i64>()
                        .unwrap_or_else(|_| panic!("Can't parse arg #{} as i64", program_args[i])),
                ),
                ValueType::F32 => RuntimeValue::F32(
                    program_args[i]
                        .parse::<f32>()
                        .unwrap_or_else(|_| panic!("Can't parse arg #{} as f32", program_args[i]))
                        .into(),
                ),
                ValueType::F64 => RuntimeValue::F64(
                    program_args[i]
                        .parse::<f64>()
                        .unwrap_or_else(|_| panic!("Can't parse arg #{} as f64", program_args[i]))
                        .into(),
                ),
            })
            .collect::<Vec<RuntimeValue>>()
    };

    let loaded_module = wasmi::Module::from_parity_wasm_module(module).expect("Module to be valid");

    // Intialize deserialized module. It adds module into It expects 3 parameters:
    // - a name for the module
    // - a module declaration
    // - "main" module doesn't import native module(s) this is why we don't need to provide external native modules here
    // This test shows how to implement native module https://github.com/NikVolf/parity-wasm/blob/master/src/interpreter/tests/basics.rs#L197

    let mut profiler = TracingProfiler::new();
    dbg!(&profiler);

    let start = Instant::now();

    let main = ModuleInstance::new(&loaded_module, &ImportsBuilder::default())
        .expect("Failed to instantiate module")
        .run_start(&mut NopExternals, &mut profiler)
        .expect("Failed to run start function in module");

    let res = main.invoke_export(func_name, &args, &mut NopExternals, &mut profiler);
    let elapsed = start.elapsed();
    println!("Result: {:?}", res.expect(""));

    println!("instructions: {:?}", profiler.trace.len());

    let mut map: BTreeMap<Instruction, Vec<Instant>> = BTreeMap::new();

    let mut instr_total_ns = Duration::from_millis(0);

    let mut f=File::create("trace.csv").unwrap();

    let mut previous = None;

    let mut counts: BTreeMap<&Instruction, u64> = BTreeMap::new();

    for (instr, dur) in &profiler.trace {
        *counts.entry(instr).or_default() += 1u64;
        match previous.as_mut() {
            Some(previous) => {
                // let ret = dur.//dur.checked_duration_since(*previous);
                let ret = dur.checked_duration_since(*previous);
                f.write_all(format!("{:?},{:?},{:?}\n", instr, dur, ret).as_bytes()).unwrap();
                *previous = *dur;
            }
            None => {
                f.write_all(format!("{:?},{:?},{:?}\n", instr, dur, previous).as_bytes()).unwrap();
                previous = Some(*dur);
            }
        }
        // if let Some(previous) = previous {
        //     let dur = dur - previous;
        // }
        // // f.write_all(format!("{instr:?},{:?}\n", dur).as_bytes()).unwrap();
        map.entry(*instr).or_default().push(*dur);

    }

    let avg_time = profiler.trace.last().unwrap().1.duration_since(profiler.trace.first().unwrap().1) / profiler.trace.len() as u32;
    dbg!(&avg_time);

    // let avg_time = profiler.tra

    let max_block_gas = 3600000000000f64;
    let block_time = Duration::from_millis(16384);

    let gas = (avg_time.as_nanos() as f64) / (block_time.as_nanos() as f64) * max_block_gas;

    let interpreter_overhead = elapsed - instr_total_ns;

    println!("total invoke_export time: {}", elapsed.as_nanos());
    println!("total instruction time: {}ns", instr_total_ns.as_nanos());

    let startup_overhead = instr_total_ns.as_nanos() as f64 / elapsed.as_nanos() as f64;

    assert!(elapsed > instr_total_ns);

    // let ns = elapsed.as_nanos() - instr_total_ns.as_nanos();
    println!(
        "startup overhead: {:?}",
        interpreter_overhead,

    );
    println!("startup proportion: {}", startup_overhead);
    println!("gas = {}", gas);

    let mut verify = 0f64;
    let total_counts = counts.values().sum::<u64>();
    for (instr, count) in counts {
        let proportion = count as f64 / total_counts as f64;
        // verify += proportion;
        let adjusted_gas = gas * proportion;
        verify += adjusted_gas;
        println!("{instr:?} {proportion} adjusted gas = {adjusted_gas}", instr=instr, proportion=proportion);
    }

    dbg!(verify);
    // dbg!(verify);
// // total invoke_export time: 855602917
// // total instruction time:   283056794

    for (instr, mut durs) in map {
        // let mut avg_ns = durs.iter().sum::<Duration>().as_nanos() as f64 / durs.len() as f64;

        // assert!(durs.is_s)
        assert_eq!(durs, {
            let mut durs_sorted = durs.clone();
            durs_sorted.sort();
            durs_sorted
        });

        let avg_ns = if durs.len() >= 2 {
            durs.last().unwrap().duration_since(*durs.first().unwrap()) / durs.len().try_into().unwrap()
        } else {
            // durs.first().unwrap().du
            todo!("?");
        };

        // durs.sort();
        // // eprintln!("{:?}", durs);
        // let p75 = durs.iter().nth(durs.len() * 75 / 100).unwrap();
        // let min = durs.iter().min();
        // let max = durs.iter().max();
        let measure = avg_ns;

        // println!("{:?} min={:?} max={:?} p75={:?} measure={:?} avg={:?}", instr, min, max, p75, measure, avg_ns);
        // println!("{:?} measure={:?}", instr, measure);
        let time_share = measure.as_nanos() as f64 / (block_time.as_nanos() as f64);
        let gas = (time_share as f64) * max_block_gas;
        println!("{:?} gas={}", instr, gas);
    }

//     // println!("({}/{})*{}", interpreter_overhead.as_nanos(), block_time.as_nanos(), max_block_gas);
}

#[cfg(feature = "std")]
fn load_module(file: &str) -> Module {
    parity_wasm::deserialize_file(file).expect("File to be deserialized")
}

#[cfg(not(feature = "std"))]
fn load_module(file: &str) -> Module {
    let mut buf = std::fs::read(file).expect("Read file");
    parity_wasm::deserialize_buffer(&mut buf).expect("Deserialize module")
}
