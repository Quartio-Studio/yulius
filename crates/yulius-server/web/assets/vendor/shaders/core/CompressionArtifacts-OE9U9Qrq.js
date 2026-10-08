import { Ac as defineStd, Bd as ZERO, Ec as vec2, Gc as createGridKernelPass, Gs as clamp, Hd as call, Jd as vec4, Ml as p, Qs as floor, Rc as buildBlockQuantizeGraph, Ws as add, dc as mul, if as unpremultiplyAlpha, oc as local, qc as makeJpegBlockRgb, qs as div } from "./std-ClftfWpF.js";
var N_LUMA = 6;
var SCALE_N = 4;
var STRIDE = SCALE_N;
var MAX_CELLS = 1024;
var CELL_FORMAT = "rgba16float";
const buildCompressionGraph = () => buildBlockQuantizeGraph({
	nLuma: N_LUMA,
	scaleN: SCALE_N,
	format: CELL_FORMAT,
	namePrefix: "compression"
});
const jpegBlockRgb = makeJpegBlockRgb(N_LUMA);
function cellFetchComposite(cellTex, sharp, params) {
	const { ctx } = params;
	const center = call(unpremultiplyAlpha, "unpremultiplyAlpha", [sharp.sample(ctx.uv)]);
	const pPix = local(floor(mul(ctx.uv, ctx.viewportSize)), "pPix");
	const cellX = clamp(floor(div(pPix.member("x"), STRIDE)), 0, MAX_CELLS - 1);
	const cellY = clamp(floor(div(pPix.member("y"), STRIDE)), 0, MAX_CELLS - 1);
	const cellUV = vec2(div(add(cellX, .5), MAX_CELLS), div(add(cellY, .5), MAX_CELLS));
	return vec4(cellTex.sample(cellUV, "nearestClamp").member("rgb"), center.member("a"));
}
function jpegCompress(opts) {
	return {
		compute: (params) => {
			const { childNode, gpu, convertToTexture, registerComputeTexture, getCpuValue, onCleanup, onResize, dimensions } = params;
			if (!childNode) return null;
			const root = gpu?.root;
			if (!root) return null;
			const childTexture = convertToTexture(childNode);
			const cellTex = root.createTexture({
				size: [MAX_CELLS, MAX_CELLS],
				format: CELL_FORMAT
			}).$usage("storage", "sampled");
			onCleanup(() => cellTex.destroy());
			const jpegCellTexture = registerComputeTexture(cellTex);
			const graph = buildCompressionGraph();
			const compParams = root.createUniform(graph.Params);
			let curW = Math.max(1, Math.round(dimensions.width));
			let curH = Math.max(1, Math.round(dimensions.height));
			const cellsFor = (w, h) => [Math.min(Math.ceil(w / STRIDE), MAX_CELLS), Math.min(Math.ceil(h / STRIDE), MAX_CELLS)];
			let [cellsX, cellsY] = cellsFor(curW, curH);
			const pipeline = createGridKernelPass(root, graph.kernel);
			let bound = pipeline;
			let inputBound = false;
			onResize(({ width, height }) => {
				curW = Math.max(1, Math.round(width));
				curH = Math.max(1, Math.round(height));
				[cellsX, cellsY] = cellsFor(curW, curH);
			});
			return {
				outputs: {
					childTexture,
					jpegCellTexture
				},
				bindInputs: (resolve) => {
					const src = resolve(childTexture.key);
					if (src) {
						const bg = root.createBindGroup(graph.layout, {
							input: src.texture,
							cellTex,
							params: compParams.buffer
						});
						bound = pipeline.with(bg);
						inputBound = true;
					}
				},
				getComputeNodes: () => {
					const q = getCpuValue(opts.quality.name);
					compParams.write({
						inputWidth: curW,
						inputHeight: curH,
						quality: typeof q === "number" ? q : 12
					});
					return [() => {
						if (inputBound) bound.dispatchThreads(cellsX, cellsY);
					}];
				}
			};
		},
		gpu: { fragment: (params) => {
			const { childNode, ctx, computeOutputs, convertToTexture } = params;
			if (!childNode) return ZERO;
			const cellTex = computeOutputs?.jpegCellTexture;
			const sharp = computeOutputs?.childTexture;
			if (!cellTex || !sharp) return call(unpremultiplyAlpha, "unpremultiplyAlpha", [convertToTexture(childNode).sample(ctx.uv)]);
			return cellFetchComposite(cellTex, sharp, params);
		} }
	};
}
const componentDefinition = defineStd({
	name: "CompressionArtifacts",
	role: "filter",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Simulates lossy JPEG compression — 8×8 DCT block quantization, blockiness, ringing and color bleed",
	requiresRTT: true,
	requiresChild: true,
	props: { quality: {
		default: 12,
		description: "JPEG quality (1 = heavily destroyed, 100 = near lossless)",
		ui: {
			type: ["range", "map"],
			min: 1,
			max: 100,
			step: 1,
			label: "Quality",
			group: "Effect"
		}
	} },
	...jpegCompress({ quality: p("quality") })
});
var CompressionArtifacts_default = componentDefinition;
export { jpegBlockRgb as i, buildCompressionGraph as n, componentDefinition as r, CompressionArtifacts_default as t };
