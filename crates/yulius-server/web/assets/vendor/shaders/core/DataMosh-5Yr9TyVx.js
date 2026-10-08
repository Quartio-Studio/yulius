import { Ac as defineStd, Bd as ZERO, Hd as call, Vc as buildMacroblockAdvectKernel, g as feedbackSim, if as unpremultiplyAlpha, qd as mixExpr } from "./std-ClftfWpF.js";
var STATE_RES = 768;
var STATE_FORMAT = "rgba16float";
var decoder = buildMacroblockAdvectKernel({
	res: STATE_RES,
	format: STATE_FORMAT,
	namePrefix: "dataMosh",
	blockMix: {
		coarseScale: 2.6,
		coarsePick: .35
	},
	generationLoss: {
		levels: 18,
		amount: .12,
		decay: .9985
	}
});
const moshKernel = decoder.kernel;
const componentDefinition = defineStd({
	name: "DataMosh",
	role: "simulation",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Corrupted-codec motion smearing — macroblocks lock in place and drag their pixels across the frame in liquid trails, each recovering on its own clock, like a video stream that lost its keyframes",
	requiresRTT: true,
	requiresChild: true,
	props: {
		intensity: {
			default: .7,
			description: "How much of the image locks and smears — the fraction of corrupted macroblocks",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Intensity",
				group: "Mosh"
			}
		},
		blockSize: {
			default: 48,
			description: "Macroblock size — the granularity of the corruption",
			ui: {
				type: "range",
				min: 30,
				max: 150,
				step: 1,
				label: "Block Size",
				group: "Mosh"
			}
		},
		drift: {
			default: .35,
			description: "Motion vector strength — how fast corrupted areas drag their pixels away",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Drift",
				group: "Mosh"
			}
		},
		churn: {
			default: .4,
			description: "How often each block re-rolls its fate — low values let corrupted regions melt for a long time before recovering",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Churn",
				group: "Mosh"
			}
		},
		blend: {
			default: 1,
			description: "Blend between the live image and the moshed stream",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Blend",
				group: "Mosh"
			}
		},
		speed: {
			default: 1,
			description: "Simulation speed. 0 freezes the decay in place.",
			ui: {
				type: "range",
				min: 0,
				max: 4,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		},
		seed: {
			default: 0,
			description: "Random seed for the corruption layout",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Animation"
			}
		}
	},
	...feedbackSim((params, root) => {
		const { getCpuValue } = params;
		return {
			size: STATE_RES,
			format: STATE_FORMAT,
			speedProp: "speed",
			paramsSchema: decoder.Params,
			bindGroups: ({ childTexture, display, paramsBuffer }, read, write) => root.createBindGroup(decoder.layout, {
				src: childTexture,
				prev: read.state,
				next: write.state,
				display,
				params: paramsBuffer
			}),
			step: moshKernel,
			values: ({ dt, localTime }) => ({
				time: localTime,
				dt,
				seed: getCpuValue("seed") ?? 0,
				intensity: getCpuValue("intensity") ?? .7,
				blockSize: getCpuValue("blockSize") ?? 48,
				drift: getCpuValue("drift") ?? .35,
				churn: getCpuValue("churn") ?? .4
			})
		};
	}),
	gpu: { fragment: ({ childNode, ctx, computeOutputs, uniforms, convertToTexture }) => {
		if (!childNode) return ZERO;
		const display = computeOutputs?.display;
		const live = (computeOutputs?.childTexture ?? convertToTexture(childNode)).sample(ctx.uv);
		if (!display) return call(unpremultiplyAlpha, "unpremultiplyAlpha", [live]);
		return call(unpremultiplyAlpha, "unpremultiplyAlpha", [mixExpr(live, display.sample(ctx.uv, "linearClamp"), uniforms.blend)]);
	} }
});
var DataMosh_default = componentDefinition;
export { componentDefinition as n, moshKernel as r, DataMosh_default as t };
