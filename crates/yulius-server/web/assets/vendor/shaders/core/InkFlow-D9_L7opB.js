import { $f as f32, Ac as defineStd, Bd as ZERO, Gs as clamp, Jd as vec4, Jf as textureStorage2d, Wf as vec4f, Xf as struct, Yf as arrayOf, cc as max, cl as buildBrushSplatKernel, d as cursorRibbon, dc as mul, f as fluidSim, oc as local, of as convertP3ToMixSpaceCPU, pl as buildStableFluidsKernels, sd as decayFadeSeconds, ul as buildFluidOutputKernel, xf as typegpu_default } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var N = 256;
var COUNT = N * N;
var JACOBI_ITERS = 10;
var STATE_FORMAT = "rgba16float";
var IMPULSE_K = .16;
var MAX_STEPS = 16;
var CYCLE_PER_SPLAT = .006;
var CYCLE_TIME_RATE = .06;
var VEL_FADE_MAX = 4;
var VEL_FADE_MIN = .05;
var GAIN = 1.35;
var FlowParams = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(struct({
	dt: f32,
	curlStrength: f32,
	velFade: f32,
	dyeFade: f32
}), "FlowParams");
var SplatParams = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(struct({
	posX: f32,
	posY: f32,
	velX: f32,
	velY: f32,
	colR: f32,
	colG: f32,
	colB: f32,
	col2R: f32,
	col2G: f32,
	col2B: f32,
	mixT: f32,
	radius: f32,
	strength: f32
}), "SplatParams");
var fluidLayout = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(typegpu_default.bindGroupLayout({
	velA: {
		storage: arrayOf(vec4f, COUNT),
		access: "mutable"
	},
	velB: {
		storage: arrayOf(vec4f, COUNT),
		access: "mutable"
	},
	dyeA: {
		storage: arrayOf(vec4f, COUNT),
		access: "mutable"
	},
	dyeB: {
		storage: arrayOf(vec4f, COUNT),
		access: "mutable"
	},
	pressure: {
		storage: arrayOf(f32, COUNT),
		access: "mutable"
	},
	divergence: {
		storage: arrayOf(f32, COUNT),
		access: "mutable"
	},
	params: { uniform: FlowParams }
}), "fluidLayout");
var splatLayout = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(typegpu_default.bindGroupLayout({
	velA: {
		storage: arrayOf(vec4f, COUNT),
		access: "mutable"
	},
	dyeA: {
		storage: arrayOf(vec4f, COUNT),
		access: "mutable"
	},
	sparams: { uniform: SplatParams }
}), "splatLayout");
var outputLayout = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(typegpu_default.bindGroupLayout({
	dyeA: {
		storage: arrayOf(vec4f, COUNT),
		access: "readonly"
	},
	outTex: { storageTexture: textureStorage2d(STATE_FORMAT, "write-only") }
}), "outputLayout");
var makeSplatKernel = (colorSpaceMode) => buildBrushSplatKernel(splatLayout, {
	n: N,
	namePrefix: "inkFlow",
	colorSpace: colorSpaceMode
});
var splatKernelVariants = {
	0: makeSplatKernel(0),
	1: makeSplatKernel(1),
	2: makeSplatKernel(2),
	3: makeSplatKernel(3),
	4: makeSplatKernel(4),
	5: makeSplatKernel(5)
};
const splatKernel = splatKernelVariants[2];
var solverKernels = buildStableFluidsKernels(fluidLayout, {
	n: N,
	namePrefix: "inkFlow",
	boundary: "clamped",
	dye: "rgb",
	dyeDissipation: true
});
const { curl: curlKernel, vorticity: vorticityKernel, divergence: divergenceKernel, jacobi: jacobiKernel, gradSubtract: gradSubtractKernel, advectVel: advectVelKernel, copyVel: copyVelKernel, advectDye: advectDyeKernel, copyDye: copyDyeKernel } = solverKernels;
const outputKernel = buildFluidOutputKernel(outputLayout, {
	n: N,
	namePrefix: "inkFlow",
	dye: "rgb"
});
function srgbToLinear(c) {
	return c <= .04045 ? c / 12.92 : Math.pow((c + .055) / 1.055, 2.4);
}
function hueToLinearRgb(hue) {
	const h = (hue % 1 + 1) % 1;
	const i = Math.floor(h * 6);
	const f = h * 6 - i;
	const q = 1 - f;
	let r = 0;
	let g = 0;
	let b = 0;
	if (i === 0) {
		r = 1;
		g = f;
		b = 0;
	} else if (i === 1) {
		r = q;
		g = 1;
		b = 0;
	} else if (i === 2) {
		r = 0;
		g = 1;
		b = f;
	} else if (i === 3) {
		r = 0;
		g = q;
		b = 1;
	} else if (i === 4) {
		r = f;
		g = 0;
		b = 1;
	} else {
		r = 1;
		g = 0;
		b = q;
	}
	return [
		srgbToLinear(r),
		srgbToLinear(g),
		srgbToLinear(b)
	];
}
const componentDefinition = defineStd({
	name: "InkFlow",
	role: "simulation",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Interactive",
	description: "Drag to paint swirling ribbons of ink through a real fluid field — eddies keep evolving after you let go",
	requiresRTT: false,
	requiresChild: false,
	usesPointer: true,
	props: {
		colorMode: {
			default: "rainbow",
			description: "How the brush color is chosen: an ever-cycling rainbow, a single fixed color, or a custom three-color cycle",
			ui: {
				type: "select",
				options: [
					{
						label: "Rainbow",
						value: "rainbow"
					},
					{
						label: "Single Color",
						value: "single"
					},
					{
						label: "Custom Colors",
						value: "custom"
					}
				],
				label: "Color Mode",
				group: "Color"
			}
		},
		colorSpeed: {
			default: 1,
			description: "How quickly the brush cycles through the rainbow or the custom colors as you paint",
			ui: {
				type: "range",
				min: 0,
				max: 4,
				step: .05,
				label: "Color Speed",
				group: "Color",
				condition: { colorMode: ["rainbow", "custom"] }
			}
		},
		color: {
			default: "#ff2d7e",
			transform: transformColor,
			description: "Brush color used when Color Mode is Single",
			ui: {
				type: "color",
				label: "Brush Color",
				group: "Color",
				condition: { colorMode: "single" }
			}
		},
		color1: {
			default: "#4338ff",
			transform: transformColor,
			description: "First color of the custom brush cycle",
			ui: {
				type: "color",
				label: "Color 1",
				group: "Color",
				condition: { colorMode: "custom" }
			}
		},
		color2: {
			default: "#ff2d7e",
			transform: transformColor,
			description: "Second color of the custom brush cycle",
			ui: {
				type: "color",
				label: "Color 2",
				group: "Color",
				condition: { colorMode: "custom" }
			}
		},
		color3: {
			default: "#19e3ff",
			transform: transformColor,
			description: "Third color of the custom brush cycle",
			ui: {
				type: "color",
				label: "Color 3",
				group: "Color",
				condition: { colorMode: "custom" }
			}
		},
		colorSpace: {
			default: "oklab",
			transform: transformColorSpace,
			compileTime: true,
			description: "color space the custom colors blend through as the brush cycles between them",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Color",
				condition: { colorMode: "custom" }
			}
		},
		radius: {
			default: .3,
			description: "Size of the ink splat painted under the cursor",
			ui: {
				type: "range",
				min: .1,
				max: 5,
				step: .05,
				label: "Radius",
				group: "Brush"
			}
		},
		force: {
			default: 1,
			description: "How hard the cursor's motion pushes the fluid — higher throws faster, more violent jets and curls",
			ui: {
				type: "range",
				min: 0,
				max: 3,
				step: .05,
				label: "Force",
				group: "Brush"
			}
		},
		curl: {
			default: 0,
			description: "Vorticity confinement — swirl energy that spins the ink into persistent eddies and curls",
			ui: {
				type: "range",
				min: 0,
				max: 60,
				step: 1,
				label: "Swirl",
				group: "Dynamics"
			}
		},
		decay: {
			default: .5,
			description: "How fast the ink fades away — low values leave long, lingering trails",
			ui: {
				type: "range",
				min: .05,
				max: 4,
				step: .05,
				label: "Decay",
				group: "Dynamics"
			}
		},
		momentum: {
			default: .6,
			description: "How long the fluid keeps flowing after a stroke — high momentum lets the motion carry on and on",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Momentum",
				group: "Dynamics"
			}
		}
	},
	...fluidSim((params, root) => {
		const { getCpuValue, propValues } = params;
		const spaceMode = propValues.colorSpace ?? 2;
		const activeSplatKernel = splatKernelVariants[spaceMode] ?? splatKernelVariants[2];
		const splatU = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(root.createUniform(SplatParams), "splatU");
		let hueCursor = 0;
		const toRgb = (v, fallback) => {
			const c = v;
			return c && typeof c.r === "number" ? [
				c.r,
				c.g ?? 0,
				c.b ?? 0
			] : fallback;
		};
		const conv = (c) => convertP3ToMixSpaceCPU(c[0], c[1], c[2], spaceMode);
		const strokes = cursorRibbon({
			activeWhen: (ptr) => ptr.moving,
			fadeSeconds: (f) => decayFadeSeconds(64, f.num("decay", .5)),
			onFrame: (f) => {
				hueCursor += f.dt * CYCLE_TIME_RATE * f.num("colorSpeed", 1);
			},
			pass: { setup: (ctx) => {
				const splatBg = ctx.root.createBindGroup(splatLayout, {
					velA: ctx.buffers.velA,
					dyeA: ctx.buffers.dyeA,
					sparams: splatU.buffer
				});
				return ctx.pass(activeSplatKernel, splatBg);
			} },
			ribbon: (f, ptr) => {
				const force = f.num("force", 1);
				const radiusGrid = Math.max(f.num("radius", .3) * .1 * N, 1);
				const colorSpeed = f.num("colorSpeed", 1);
				const mode = getCpuValue("colorMode") || "rainbow";
				const customStops = mode === "custom" ? [
					conv(toRgb(getCpuValue("color1"), [
						.26,
						.22,
						1
					])),
					conv(toRgb(getCpuValue("color2"), [
						1,
						.18,
						.49
					])),
					conv(toRgb(getCpuValue("color3"), [
						.1,
						.89,
						1
					]))
				] : null;
				const brushSplat = () => {
					hueCursor += CYCLE_PER_SPLAT * colorSpeed;
					if (customStops) {
						const h = (hueCursor % 1 + 1) % 1;
						const seg = Math.min(2, Math.floor(h * 3));
						return {
							a: customStops[seg],
							b: customStops[(seg + 1) % 3],
							t: h * 3 - seg
						};
					}
					if (mode === "single") {
						const c$1 = conv(toRgb(getCpuValue("color"), [
							1,
							.18,
							.49
						]));
						return {
							a: c$1,
							b: c$1,
							t: 0
						};
					}
					const c = conv(hueToLinearRgb(hueCursor));
					return {
						a: c,
						b: c,
						t: 0
					};
				};
				const velX = ptr.velX * N * force * IMPULSE_K;
				const velY = ptr.velY * N * force * IMPULSE_K;
				return {
					stepSize: Math.max(.004, radiusGrid / N * .6),
					maxSteps: MAX_STEPS,
					prepare: brushSplat,
					write: (posX, posY, _t, bc) => splatU.write({
						posX,
						posY,
						velX,
						velY,
						colR: bc.a[0],
						colG: bc.a[1],
						colB: bc.a[2],
						col2R: bc.b[0],
						col2G: bc.b[1],
						col2B: bc.b[2],
						mixT: bc.t,
						radius: radiusGrid,
						strength: 1
					})
				};
			}
		});
		return {
			resolution: N,
			layout: fluidLayout,
			outputLayout,
			paramsSchema: FlowParams,
			solver: {
				kernels: solverKernels,
				jacobiIters: JACOBI_ITERS
			},
			output: {
				kernel: outputKernel,
				key: "dyeTexture"
			},
			inject: [strokes],
			values: (f) => {
				const momentum = Math.min(Math.max(f.num("momentum", .6), 0), 1);
				return {
					dt: f.dt,
					curlStrength: f.num("curl", 0),
					velFade: VEL_FADE_MAX * Math.pow(VEL_FADE_MIN / VEL_FADE_MAX, momentum),
					dyeFade: f.num("decay", .5)
				};
			}
		};
	}),
	gpu: { fragment: ({ ctx, computeOutputs }) => {
		const dyeField = computeOutputs?.dyeTexture;
		if (!dyeField) return ZERO;
		const dye = local(dyeField.sample(ctx.uv, "linearClamp").member("rgb"), "inkDye");
		return vec4(dye, clamp(mul(max(dye.member("x"), max(dye.member("y"), dye.member("z"))), GAIN), 0, 1));
	} }
});
var InkFlow_default = componentDefinition;
export { copyDyeKernel as a, divergenceKernel as c, outputKernel as d, splatKernel as f, componentDefinition as i, gradSubtractKernel as l, advectDyeKernel as n, copyVelKernel as o, vorticityKernel as p, advectVelKernel as r, curlKernel as s, InkFlow_default as t, jacobiKernel as u };
