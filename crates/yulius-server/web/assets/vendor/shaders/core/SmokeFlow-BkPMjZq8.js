import { $f as f32, Ac as defineStd, Bd as ZERO, Ed as mixColorStopsRuntime, Hd as call, Jd as vec4, Jf as textureStorage2d, Wf as vec4f, Xf as struct, Yf as arrayOf, af as colorMixing_exports, d as cursorRibbon, f as fluidSim, ll as buildEmitterSplatKernel, ou as colorStopsPropConfig, pl as buildStableFluidsKernels, sd as decayFadeSeconds, ul as buildFluidOutputKernel, xf as typegpu_default } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var N = 256;
var COUNT = N * N;
var JACOBI_ITERS = 10;
var VELOCITY_DISSIPATION = .2;
var MAX_STEPS = 16;
var STATE_FORMAT = "rgba16float";
var cm = colorMixing_exports;
var FlowParams = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(struct({
	dt: f32,
	emitX: f32,
	emitY: f32,
	emitVelX: f32,
	emitVelY: f32,
	emitGate: f32,
	emitRad: f32,
	emitIntensity: f32,
	dyeFade: f32,
	velFade: f32,
	curlStrength: f32,
	gravity: f32,
	colorDecay: f32
}), "FlowParams");
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
var outputLayout = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(typegpu_default.bindGroupLayout({
	dyeA: {
		storage: arrayOf(vec4f, COUNT),
		access: "readonly"
	},
	outTex: { storageTexture: textureStorage2d(STATE_FORMAT, "write-only") }
}), "outputLayout");
const splatKernel = buildEmitterSplatKernel(fluidLayout, {
	n: N,
	namePrefix: "smokeFlow",
	cone: false,
	cursorPush: false,
	gated: true,
	densityGain: 10,
	velocityBlendGain: 4
});
var solverKernels = buildStableFluidsKernels(fluidLayout, {
	n: N,
	namePrefix: "smokeFlow",
	boundary: "clamped",
	dye: "densityAge",
	dyeDissipation: true,
	ageAdvance: true
});
const { curl: curlKernel, vorticity: vorticityKernel, divergence: divergenceKernel, jacobi: jacobiKernel, gradSubtract: gradSubtractKernel, advectVel: advectVelKernel, copyVel: copyVelKernel, advectDye: advectDensKernel, copyDye: copyDensKernel } = solverKernels;
const outputKernel = buildFluidOutputKernel(outputLayout, {
	n: N,
	namePrefix: "smokeFlow",
	dye: "densityAge"
});
const componentDefinition = defineStd({
	name: "SmokeFlow",
	role: "simulation",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Interactive",
	description: "Cursor-driven smoke that lingers, swirls, and dissipates with fluid dynamics",
	usesPointer: true,
	props: {
		colorA: {
			default: "#e29c8b",
			transform: transformColor,
			description: "Color of fresh smoke",
			ui: {
				type: "color",
				label: "Fresh Color",
				group: "Colors"
			}
		},
		colorB: {
			default: "#d517f9",
			transform: transformColor,
			description: "Color smoke transitions to as it ages",
			ui: {
				type: "color",
				label: "Aged Color",
				group: "Colors"
			}
		},
		stops: colorStopsPropConfig(),
		intensity: {
			default: 1,
			description: "How much smoke is emitted as you move the cursor",
			ui: {
				type: "range",
				min: .1,
				max: 2,
				step: .05,
				label: "Intensity",
				group: "Emission"
			}
		},
		emitRadius: {
			default: .07,
			description: "Size of smoke puff emitted at cursor",
			ui: {
				type: "range",
				min: .01,
				max: .3,
				step: .01,
				label: "Puff Size",
				group: "Emission"
			}
		},
		momentum: {
			default: 20,
			description: "How much cursor velocity is transferred into the smoke (higher = more directed flow)",
			ui: {
				type: "range",
				min: 0,
				max: 50,
				step: 1,
				label: "Momentum",
				group: "Emission"
			}
		},
		dissipation: {
			default: .5,
			description: "How fast smoke fades over time",
			ui: {
				type: "range",
				min: .05,
				max: 3,
				step: .05,
				label: "Dissipation",
				group: "Effect"
			}
		},
		detail: {
			default: 10,
			description: "Fine-scale swirling detail driven by vorticity confinement",
			ui: {
				type: "range",
				min: 0,
				max: 60,
				step: 1,
				label: "Swirl Detail",
				group: "Effect"
			}
		},
		gravity: {
			default: 2,
			description: "Vertical drift — negative floats up, positive sinks down",
			ui: {
				type: "range",
				min: -5,
				max: 5,
				step: .05,
				label: "Gravity",
				group: "Effect"
			}
		},
		colorDecay: {
			default: .5,
			description: "How quickly smoke shifts from fresh to aged color",
			ui: {
				type: "range",
				min: 0,
				max: 3,
				step: .1,
				label: "Color Decay",
				group: "Colors"
			}
		},
		colorSpace: {
			default: "OKLAB",
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
		const speedGateOf = (smoothSpeed) => Math.min(smoothSpeed * smoothSpeed * 60, 1);
		const strokes = cursorRibbon({
			tracker: { teleportGuard: Number.POSITIVE_INFINITY },
			activeWhen: (ptr) => speedGateOf(ptr.smoothSpeed) > .005,
			fadeSeconds: (f) => decayFadeSeconds(255, f.num("dissipation", .4)),
			pass: { kernel: splatKernel },
			ribbon: (f, _ptr, values) => ({
				stepSize: Math.max(.004, f.num("emitRadius", .07) * .5),
				maxSteps: MAX_STEPS,
				write: (emitX, emitY) => f.writeParams({
					...values,
					emitX,
					emitY
				})
			})
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
				key: "smokeTexture"
			},
			inject: [strokes],
			values: (f) => {
				const ptr = strokes.ptr();
				const momentumScale = f.num("momentum", 1.2);
				const speedGate = speedGateOf(ptr.smoothSpeed);
				const isActive = strokes.active();
				const emitRadiusUv = f.num("emitRadius", .07);
				return {
					dt: f.dt,
					emitX: ptr.x * N,
					emitY: ptr.y * N,
					emitGate: isActive ? speedGate : 0,
					emitVelX: ptr.smoothVelX * N * momentumScale * .12,
					emitVelY: ptr.smoothVelY * N * momentumScale * .12,
					emitRad: emitRadiusUv * N,
					emitIntensity: f.num("intensity", 1.2),
					dyeFade: f.num("dissipation", .4),
					velFade: VELOCITY_DISSIPATION,
					curlStrength: f.num("detail", 28),
					gravity: f.num("gravity", -.3),
					colorDecay: f.num("colorDecay", .5)
				};
			}
		};
	}),
	gpu: { fragment: ({ ctx, computeOutputs, propValues, uniforms }) => {
		const smokeField = computeOutputs?.smokeTexture;
		if (!smokeField) return ZERO;
		const s = smokeField.sample(ctx.uv, "linearClamp");
		const smokeDensity = s.member("x");
		const smokeAge = s.member("y");
		const colorSpaceMode = propValues.colorSpace ?? 0;
		const stopCount = propValues.stopCount ?? 0;
		let smokeColor;
		if (stopCount > 1) smokeColor = mixColorStopsRuntime(smokeAge, {
			colorsArray: uniforms.colorsArray,
			positionsArray: uniforms.positionsArray,
			convertedColorsArray: uniforms.convertedColorsArray,
			stopCount: uniforms.stopCount
		}, colorSpaceMode);
		else smokeColor = call(cm.mixColorsVariants[colorSpaceMode] ?? cm.mixColorsLinear, "mixColors", [
			uniforms.colorA,
			uniforms.colorB,
			smokeAge
		]);
		return vec4(smokeColor.member("rgb"), smokeColor.member("a").mul(smokeDensity));
	} }
});
var SmokeFlow_default = componentDefinition;
export { copyDensKernel as a, divergenceKernel as c, outputKernel as d, splatKernel as f, componentDefinition as i, gradSubtractKernel as l, advectDensKernel as n, copyVelKernel as o, vorticityKernel as p, advectVelKernel as r, curlKernel as s, SmokeFlow_default as t, jacobiKernel as u };
