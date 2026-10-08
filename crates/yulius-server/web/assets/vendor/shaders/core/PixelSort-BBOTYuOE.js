import { $f as f32, Ac as defineStd, Bd as ZERO, Ec as vec2, Gc as createGridKernelPass, Hc as buildOddEvenSortSet, Hd as call, Pd as createStateBuffer, at as trackedViewport, et as gridSim, if as unpremultiplyAlpha, it as op } from "./std-ClftfWpF.js";
var WORK = 512;
var CELL_COUNT = WORK * WORK;
var STATE_FORMAT = "rgba16float";
const makePixelSortKernels = (vertical, direction) => buildOddEvenSortSet({
	n: WORK,
	format: STATE_FORMAT,
	namePrefix: "pixelSort",
	vertical,
	direction
});
const componentDefinition = defineStd({
	name: "PixelSort",
	role: "simulation",
	species: "custom",
	category: "Interactive",
	description: "Pixels sort by brightness around the cursor and keep their sorted position, optionally decaying back over time",
	requiresRTT: true,
	requiresChild: true,
	boundingBoxDeclaration: { aspectRatio: null },
	usesPointer: true,
	props: {
		radius: {
			default: .4,
			description: "Size of the cursor brush that sorts pixels",
			ui: {
				type: ["range", "map"],
				min: .03,
				max: .6,
				step: .01,
				label: "Radius",
				group: "Effect"
			}
		},
		falloff: {
			default: 1,
			description: "Softness of the brush edge (0 = hard circle, 1 = very soft)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Falloff",
				group: "Effect"
			}
		},
		strength: {
			default: .1,
			description: "How fast pixels sort — runs more sorting passes per frame",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Strength",
				group: "Effect"
			}
		},
		decay: {
			default: .1,
			description: "How quickly sorted pixels melt back to their original position (0 = permanent)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Decay",
				group: "Effect"
			}
		},
		axis: {
			default: "vertical",
			compileTime: true,
			description: "Axis pixels are sorted along",
			ui: {
				type: "select",
				options: [{
					label: "Horizontal",
					value: "horizontal"
				}, {
					label: "Vertical",
					value: "vertical"
				}],
				label: "Axis",
				group: "Effect"
			}
		},
		direction: {
			default: "descending",
			compileTime: true,
			description: "Sort order by brightness",
			ui: {
				type: "select",
				options: [{
					label: "Dark → Light",
					value: "ascending"
				}, {
					label: "Light → Dark",
					value: "descending"
				}],
				label: "Direction",
				group: "Effect"
			}
		}
	},
	...gridSim((params, root) => {
		const { childNode, convertToTexture, registerComputeTexture, propValues, onCleanup } = params;
		if (!childNode) return null;
		const vertical = propValues.axis === "vertical";
		const dir = propValues.direction === "descending" ? -1 : 1;
		const childTexture = convertToTexture(childNode);
		const bufferA = createStateBuffer(root, f32, CELL_COUNT);
		const bufferB = createStateBuffer(root, f32, CELL_COUNT);
		const lumaBuf = createStateBuffer(root, f32, CELL_COUNT);
		const stateTex = root.createTexture({
			size: [WORK, WORK],
			format: STATE_FORMAT
		}).$usage("storage", "sampled");
		onCleanup(() => stateTex.destroy());
		const state = registerComputeTexture(stateTex);
		const set = makePixelSortKernels(vertical, dir);
		const lumaParams = root.createUniform(set.LumaParams);
		const swapParams = root.createUniform(set.SwapParams);
		const viewport = trackedViewport(params);
		const size = [WORK, WORK];
		let lumaPipeline = createGridKernelPass(root, set.key, { size });
		let lumaReady = false;
		const swapBgAB = root.createBindGroup(set.swapLayout, {
			readBuf: bufferA,
			writeBuf: bufferB,
			lumaBuf,
			params: swapParams.buffer
		});
		const swapBgBA = root.createBindGroup(set.swapLayout, {
			readBuf: bufferB,
			writeBuf: bufferA,
			lumaBuf,
			params: swapParams.buffer
		});
		const swap0 = createGridKernelPass(root, set.swap0, { size });
		const swap1 = createGridKernelPass(root, set.swap1, { size });
		const outputBgA = root.createBindGroup(set.outputLayout, {
			srcBuf: bufferA,
			stateTex
		});
		const outputBgB = root.createBindGroup(set.outputLayout, {
			srcBuf: bufferB,
			stateTex
		});
		const outputPass = createGridKernelPass(root, set.output, { size });
		let frame = 0;
		let passes = 1;
		const sort = op.sortPass({
			passes: () => passes,
			pass: (parity, side) => (parity ? swap1 : swap0).with(side === "A" ? swapBgAB : swapBgBA)
		});
		return {
			outputs: {
				state,
				childTexture
			},
			bindInputs: (resolve) => {
				const src = resolve(childTexture.key);
				if (src) {
					const lumaBg = root.createBindGroup(set.lumaLayout, {
						input: src.texture,
						lumaBuf,
						params: lumaParams.buffer
					});
					lumaPipeline = lumaPipeline.with(lumaBg);
					lumaReady = true;
				}
			},
			clampDt: .05,
			stages: [
				op.readyWhen(() => lumaReady),
				op.values("brush+decay", (f) => {
					const fp = f.frameParams;
					const safe = viewport.safe(fp);
					const tracked = viewport.tracked();
					const strength = f.num("strength", .1);
					const decayProp = f.num("decay", .1);
					passes = 1 + Math.round(strength * 4);
					swapParams.write({
						mouseX: fp.pointer?.x ?? .5,
						mouseY: fp.pointer?.y ?? .5,
						radius: f.num("radius", .4),
						falloff: f.num("falloff", 1),
						decay: decayProp * f.dt * 2 / passes,
						aspect: safe.width / safe.height,
						seed: frame % 1024
					});
					lumaParams.write({
						inputWidth: tracked.width,
						inputHeight: tracked.height
					});
				}),
				op.cache(() => lumaPipeline),
				sort,
				op.publish(() => outputPass.with(sort.side() === "A" ? outputBgA : outputBgB)),
				op.values("frameAdvance", () => {
					frame++;
				})
			]
		};
	}),
	gpu: { fragment: ({ childNode, ctx, computeOutputs, propValues, convertToTexture }) => {
		if (!childNode) return ZERO;
		const state = computeOutputs?.state;
		const childTex = computeOutputs?.childTexture;
		if (!state || !childTex) return call(unpremultiplyAlpha, "unpremultiplyAlpha", [convertToTexture(childNode).sample(ctx.uv)]);
		const vertical = propValues.axis === "vertical";
		const srcCoord = state.sample(ctx.uv, "nearestClamp").member("r");
		const childUV = vertical ? vec2(ctx.uv.member("x"), srcCoord) : vec2(srcCoord, ctx.uv.member("y"));
		return call(unpremultiplyAlpha, "unpremultiplyAlpha", [childTex.sample(childUV)]);
	} }
});
var PixelSort_default = componentDefinition;
export { componentDefinition as n, makePixelSortKernels as r, PixelSort_default as t };
