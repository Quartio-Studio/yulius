import { $f as f32, Ac as defineStd, Bd as ZERO, Hd as call, Jd as vec4, Jf as textureStorage2d, Wf as vec4f, Xf as struct, Yf as arrayOf, af as colorMixing_exports, dl as buildNoiseFieldInitKernel, f as fluidSim, fl as buildNoiseRestoreKernel, m as seededFieldInit, ml as buildTrigTurbulenceKernel, p as restoreToward, pl as buildStableFluidsKernels, u as ambientForce, ul as buildFluidOutputKernel, xf as typegpu_default } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var N = 256;
var COUNT = N * N;
var JACOBI_ITERS = 10;
var WARM_JACOBI_ITERS = 6;
var WARM_STEPS = 50;
var VELOCITY_DISSIPATION = .15;
var INTERNAL_MAX_VEL = N * .6;
var NOISE_FREQ = 3;
var STATE_FORMAT = "rgba16float";
var cm = colorMixing_exports;
var FogParams = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(struct({
	dt: f32,
	time: f32,
	seed: f32,
	turbulence: f32,
	curlStrength: f32,
	velFade: f32,
	blending: f32,
	cursorX: f32,
	cursorY: f32,
	cursorVelX: f32,
	cursorVelY: f32,
	mouseActive: f32,
	mouseRadSq: f32
}), "FogParams");
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
	params: { uniform: FogParams }
}), "fluidLayout");
var outputLayout = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(typegpu_default.bindGroupLayout({
	dyeA: {
		storage: arrayOf(vec4f, COUNT),
		access: "readonly"
	},
	outTex: { storageTexture: textureStorage2d(STATE_FORMAT, "write-only") }
}), "outputLayout");
const initKernel = buildNoiseFieldInitKernel(fluidLayout, {
	n: N,
	namePrefix: "fog",
	frequency: NOISE_FREQ
});
const forceKernel = buildTrigTurbulenceKernel(fluidLayout, {
	n: N,
	namePrefix: "fog"
});
var solverKernels = buildStableFluidsKernels(fluidLayout, {
	n: N,
	namePrefix: "fog",
	boundary: "toroidal",
	dye: "densityAge",
	velocityCap: INTERNAL_MAX_VEL
});
const { curl: curlKernel, vorticity: vorticityKernel, divergence: divergenceKernel, jacobi: jacobiKernel, gradSubtract: gradSubtractKernel, advectVel: advectVelKernel, copyVel: copyVelKernel, advectDye: advectDensKernel, copyDye: copyDensKernel } = solverKernels;
const colorRestoreKernel = buildNoiseRestoreKernel(fluidLayout, {
	n: N,
	namePrefix: "fog",
	frequency: NOISE_FREQ
});
const outputKernel = buildFluidOutputKernel(outputLayout, {
	n: N,
	namePrefix: "fog",
	dye: "densityAge"
});
const componentDefinition = defineStd({
	name: "Fog",
	role: "simulation",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Interactive",
	description: "Fog that fills the screen and interacts with the mouse",
	usesPointer: true,
	props: {
		colorA: {
			default: "#e0e0e0",
			transform: transformColor,
			description: "Primary fog color",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#888888",
			transform: transformColor,
			description: "Secondary fog color — creates variation across the field",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		seed: {
			default: 0,
			description: "Deterministic starting pattern — different seeds produce different fog configurations",
			ui: {
				type: "range",
				min: 0,
				max: 999,
				step: 1,
				label: "Seed",
				group: "Field"
			}
		},
		speed: {
			default: 1,
			description: "Simulation speed multiplier",
			ui: {
				type: "range",
				min: .1,
				max: 3,
				step: .1,
				label: "Speed",
				group: "Field"
			}
		},
		turbulence: {
			default: 1,
			description: "Ambient motion strength",
			ui: {
				type: "range",
				min: 0,
				max: 3,
				step: .01,
				label: "Turbulence",
				group: "Field"
			}
		},
		detail: {
			default: 15,
			description: "Fine-scale swirling structure — higher values produce more intricate wisps and vortices",
			ui: {
				type: "range",
				min: 0,
				max: 50,
				step: 1,
				label: "Detail",
				group: "Field"
			}
		},
		blending: {
			default: .3,
			description: "How much the two colors blend together — 0 behaves like oil & water (colors stay distinct with sharp boundaries), 1 behaves like food coloring (colors fully mix)",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Blending",
				group: "Field"
			}
		},
		mouseInfluence: {
			default: .1,
			description: "Strength of cursor influence — move the cursor to push fog",
			ui: {
				type: "range",
				min: 0,
				max: 2,
				step: .01,
				label: "Cursor Influence",
				group: "Interaction"
			}
		},
		mouseRadius: {
			default: .1,
			description: "Radius of cursor influence area",
			ui: {
				type: "range",
				min: .02,
				max: .5,
				step: .01,
				label: "Cursor Radius",
				group: "Interaction"
			}
		},
		colorSpace: {
			default: "linear",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space for color interpolation",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		}
	},
	...fluidSim(() => {
		const fieldValues = (f) => ({
			seed: f.num("seed", 0),
			turbulence: f.num("turbulence", 1),
			curlStrength: f.num("detail", 15),
			velFade: VELOCITY_DISSIPATION,
			blending: f.num("blending", .3)
		});
		return {
			resolution: N,
			layout: fluidLayout,
			outputLayout,
			paramsSchema: FogParams,
			solver: {
				kernels: solverKernels,
				jacobiIters: JACOBI_ITERS
			},
			output: {
				kernel: outputKernel,
				key: "fogTexture"
			},
			write: "thunk",
			pointer: { minDrag: 5e-4 },
			init: seededFieldInit({
				kernel: initKernel,
				seed: (f) => f.num("seed", 0),
				warm: {
					steps: WARM_STEPS,
					jacobiIters: WARM_JACOBI_ITERS,
					dt: .1,
					startTime: () => Math.random() * 60,
					values: (t, f) => ({
						dt: .1,
						time: t,
						...fieldValues(f),
						cursorX: 0,
						cursorY: 0,
						cursorVelX: 0,
						cursorVelY: 0,
						mouseActive: 0,
						mouseRadSq: 1
					})
				}
			}),
			solve: [ambientForce(forceKernel), restoreToward(colorRestoreKernel)],
			values: (f) => {
				const mouseInf = f.num("mouseInfluence", .1);
				const mouseRadGrid = f.num("mouseRadius", .1) * N;
				const ptr = f.ptr;
				const active = ptr.moving && mouseInf > 0;
				return {
					dt: f.dt * f.num("speed", 1),
					time: f.elapsed,
					...fieldValues(f),
					cursorX: active ? ptr.x * N : 0,
					cursorY: active ? ptr.y * N : 0,
					cursorVelX: active ? ptr.dx * N * 15 * mouseInf : 0,
					cursorVelY: active ? ptr.dy * N * 15 * mouseInf : 0,
					mouseActive: active ? 1 : 0,
					mouseRadSq: mouseRadGrid * mouseRadGrid
				};
			}
		};
	}),
	gpu: { fragment: ({ ctx, computeOutputs, propValues, uniforms }) => {
		const fogField = computeOutputs?.fogTexture;
		if (!fogField) return ZERO;
		const s = fogField.sample(ctx.uv, "linearClamp");
		const fogDensity = s.member("x");
		const colorVar = s.member("y");
		const colorSpaceMode = propValues.colorSpace ?? 0;
		const fogColor = call(cm.mixColorsVariants[colorSpaceMode] ?? cm.mixColorsLinear, "mixColors", [
			uniforms.colorA,
			uniforms.colorB,
			colorVar
		]);
		return vec4(fogColor.member("rgb"), fogColor.member("a").mul(fogDensity));
	} }
});
var Fog_default = componentDefinition;
export { componentDefinition as a, curlKernel as c, gradSubtractKernel as d, initKernel as f, vorticityKernel as h, colorRestoreKernel as i, divergenceKernel as l, outputKernel as m, advectDensKernel as n, copyDensKernel as o, jacobiKernel as p, advectVelKernel as r, copyVelKernel as s, Fog_default as t, forceKernel as u };
