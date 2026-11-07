//! A network, shipped with its weights in fp16 and planned for each input
//! shape it is handed.

use std::collections::HashMap;

use tract_onnx::pb::{GraphProto, TensorProto};
use tract_onnx::prelude::*;

type Plan = TypedRunnableModel<TypedModel>;

const FLOAT: i32 = 1;
const INT8: i32 = 3;
const FLOAT16: i32 = 10;

/// A graph and the plans made from it. A plan is fixed to one shape: planned
/// for a symbolic width, the line reader runs far slower.
pub struct Network {
    graph: InferenceModel,
    plans: HashMap<Vec<usize>, Plan>,
}

impl Network {
    /// The weights stored in fp16 or in int8 are widened back to fp32: tract
    /// computes an fp16 graph far slower on a CPU, and its per-channel
    /// `DequantizeLinear` misreads the handwriting reader, many of its frames
    /// off where onnxruntime reads it as in fp32.
    pub fn parse(bytes: &[u8]) -> TractResult<Network> {
        let onnx = tract_onnx::onnx();
        let mut proto = onnx.proto_model_for_read(&mut &*bytes)?;
        let graph = proto.graph.as_mut().expect("an ONNX file holds a graph");
        graph.initializer.iter_mut().for_each(widen);
        for node in graph.node.iter_mut().filter(|n| n.op_type == "Constant") {
            node.attribute
                .iter_mut()
                .filter_map(|a| a.t.as_mut())
                .for_each(widen);
        }
        dequantize(graph);
        Ok(Network {
            graph: onnx.model_for_proto_model(&proto)?,
            plans: HashMap::new(),
        })
    }

    /// The first output for `input`, laid out as `shape`.
    pub fn run(&mut self, shape: &[usize], input: Vec<f32>) -> TractResult<Tensor> {
        if !self.plans.contains_key(shape) {
            let plan = self
                .graph
                .clone()
                .with_input_fact(0, f32::fact(shape).into())?
                .into_optimized()?
                .into_runnable()?;
            self.plans.insert(shape.to_vec(), plan);
        }
        let input = tract_ndarray::ArrayD::from_shape_vec(shape, input)?.into_tensor();
        let mut outputs = self.plans[shape].run(tvec!(input.into()))?;
        Ok(outputs.remove(0).into_tensor())
    }
}

/// Each `DequantizeLinear` of an int8 initializer, per-channel scale and zero
/// point, replaced by the fp32 initializer it computes.
fn dequantize(graph: &mut GraphProto) {
    let mut done = Vec::new();
    for (at, node) in graph.node.iter().enumerate() {
        if node.op_type != "DequantizeLinear" {
            continue;
        }
        let find = |name: &str| graph.initializer.iter().find(|t| t.name == name);
        let (Some(q), Some(scale), Some(zero)) = (
            find(&node.input[0]),
            find(&node.input[1]),
            find(&node.input[2]),
        ) else {
            continue;
        };
        if q.data_type != INT8 {
            continue;
        }
        let axis = node
            .attribute
            .iter()
            .find(|a| a.name == "axis")
            .map_or(1, |a| a.i as usize);
        let scale: Vec<f32> = scale
            .raw_data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        let zero: Vec<i8> = zero.raw_data.iter().map(|&b| b as i8).collect();
        let inner: usize = q.dims[axis + 1..].iter().product::<i64>() as usize;
        let channels = q.dims[axis] as usize;
        let data: Vec<u8> = q
            .raw_data
            .iter()
            .enumerate()
            .flat_map(|(i, &b)| {
                let c = (i / inner) % channels;
                ((b as i8 as f32 - zero[c] as f32) * scale[c]).to_le_bytes()
            })
            .collect();
        let wide = TensorProto {
            dims: q.dims.clone(),
            data_type: FLOAT,
            name: node.output[0].clone(),
            raw_data: data,
            ..TensorProto::default()
        };
        done.push((at, wide, node.input.clone()));
    }
    for (at, wide, inputs) in done.into_iter().rev() {
        graph.node.remove(at);
        graph.initializer.retain(|t| !inputs.contains(&t.name));
        graph.initializer.push(wide);
    }
}

fn widen(tensor: &mut TensorProto) {
    if tensor.data_type != FLOAT16 {
        return;
    }
    let wide: Vec<u8> = tensor
        .raw_data
        .as_chunks::<2>()
        .0
        .iter()
        .flat_map(|c| {
            f16::from_bits(u16::from_le_bytes(*c))
                .to_f32()
                .to_le_bytes()
        })
        .collect();
    tensor.raw_data = wide;
    tensor.data_type = FLOAT;
}
