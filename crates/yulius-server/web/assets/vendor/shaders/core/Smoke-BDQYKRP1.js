import { $f as f32, Ac as defineStd, Bd as ZERO, Ed as mixColorStopsRuntime, Hd as call, Jd as vec4, Jf as textureStorage2d, Wf as vec4f, Xf as struct, Yf as arrayOf, af as colorMixing_exports, f as fluidSim, h as splat, ll as buildEmitterSplatKernel, ou as colorStopsPropConfig, pl as buildStableFluidsKernels, ul as buildFluidOutputKernel, xf as typegpu_default } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var N = 256;
var COUNT = N * N;
var JACOBI_ITERS = 10;
var VELOCITY_DISSIPATION = .2;
var STATE_FORMAT = "rgba16float";
var cm = colorMixing_exports;
var FluidParams = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(struct({
	dt: f32,
	emitX: f32,
	emitY: f32,
	emitVelX: f32,
	emitVelY: f32,
	perpDirX: f32,
	perpDirY: f32,
	spreadFactor: f32,
	emitRad: f32,
	emitIntensity: f32,
	dyeFade: f32,
	velFade: f32,
	curlStrength: f32,
	gravity: f32,
	cursorX: f32,
	cursorY: f32,
	cursorVelX: f32,
	cursorVelY: f32,
	mouseActive: f32,
	mouseRadSq: f32,
	colorDecay: f32
}), "FluidParams");
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
	params: { uniform: FluidParams }
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
	namePrefix: "smoke",
	cone: true,
	cursorPush: true,
	densityGain: 8,
	velocityBlendGain: 3
});
var solverKernels = buildStableFluidsKernels(fluidLayout, {
	n: N,
	namePrefix: "smoke",
	boundary: "clamped",
	dye: "densityAge",
	dyeDissipation: true,
	ageAdvance: true
});
const { curl: curlKernel, vorticity: vorticityKernel, divergence: divergenceKernel, jacobi: jacobiKernel, gradSubtract: gradSubtractKernel, advectVel: advectVelKernel, copyVel: copyVelKernel, advectDye: advectDensKernel, copyDye: copyDensKernel } = solverKernels;
const outputKernel = buildFluidOutputKernel(outputLayout, {
	n: N,
	namePrefix: "smoke",
	dye: "densityAge"
});
const componentDefinition = defineStd({
	name: "Smoke",
	role: "simulation",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Interactive",
	description: "Realistic fluid smoke simulation with vorticity dynamics",
	usesPointer: true,
	props: {
		colorA: {
			default: "#fc83f9",
			transform: transformColor,
			description: "Color of fresh smoke",
			ui: {
				type: "color",
				label: "Start Color",
				group: "Colors"
			}
		},
		colorB: {
			default: "#c21c79",
			transform: transformColor,
			description: "Color smoke transitions to as it ages",
			ui: {
				type: "color",
				label: "End Color",
				group: "Colors"
			}
		},
		stops: colorStopsPropConfig(),
		emitFrom: {
			default: {
				x: .5,
				y: 1
			},
			transform: transformPosition,
			description: "The emission source point",
			ui: {
				type: "position",
				label: "Emit From",
				group: "Emission"
			}
		},
		direction: {
			default: 0,
			description: "Emission direction (0 = up, 90 = right, 180 = down, 270 = left)",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Direction",
				group: "Emission"
			}
		},
		speed: {
			default: 20,
			description: "Emission velocity strength",
			ui: {
				type: "range",
				min: .1,
				max: 50,
				step: .1,
				label: "Speed",
				group: "Emission"
			}
		},
		spread: {
			default: 60,
			description: "Emission cone angle in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 180,
				step: 1,
				label: "Spread",
				group: "Emission"
			}
		},
		emitRadius: {
			default: .08,
			description: "Size of the emission area",
			ui: {
				type: "range",
				min: .01,
				max: .3,
				step: .01,
				label: "Emit Size",
				group: "Emission"
			}
		},
		intensity: {
			default: 1,
			description: "Smoke emission density",
			ui: {
				type: "range",
				min: .1,
				max: 1,
				step: .01,
				label: "Intensity",
				group: "Effect"
			}
		},
		dissipation: {
			default: .2,
			description: "How fast smoke fades over time",
			ui: {
				type: "range",
				min: .1,
				max: 3,
				step: .1,
				label: "Dissipation",
				group: "Effect"
			}
		},
		detail: {
			default: 25,
			description: "Fine-scale swirling detail",
			ui: {
				type: "range",
				min: 0,
				max: 50,
				step: 1,
				label: "Detail",
				group: "Effect"
			}
		},
		gravity: {
			default: .5,
			description: "Downward gravitational pull on smoke",
			ui: {
				type: "range",
				min: -2,
				max: 2,
				step: .1,
				label: "Gravity",
				group: "Effect"
			}
		},
		colorDecay: {
			default: .4,
			description: "How quickly smoke shifts from Color A to Color B",
			ui: {
				type: "range",
				min: 0,
				max: 3,
				step: .1,
				label: "Color Decay",
				group: "Colors"
			}
		},
		mouseInfluence: {
			default: .1,
			description: "Strength of cursor influence",
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
	...fluidSim(() => ({
		resolution: N,
		layout: fluidLayout,
		outputLayout,
		paramsSchema: FluidParams,
		solver: {
			kernels: solverKernels,
			jacobiIters: JACOBI_ITERS
		},
		output: {
			kernel: outputKernel,
			key: "smokeTexture"
		},
		pointer: { minDrag: 5e-4 },
		inject: [splat({ kernel: splatKernel })],
		values: (f) => {
			const speed = f.num("speed", 3);
			const dirRad = f.num("direction", 0) * Math.PI / 180;
			const spreadHalf = Math.min(89 * Math.PI / 180, f.num("spread", 45) * Math.PI / 360);
			const emitPos = f.getCpuValue("emitFrom");
			const epx = emitPos?.x ?? .5;
			const epy = emitPos?.y ?? 0;
			const velMag = speed * N * .15;
			const mouseInf = f.num("mouseInfluence", .5);
			const mouseRad = f.num("mouseRadius", .12);
			const ptr = f.ptr;
			const mouseRadGrid = mouseRad * N;
			const active = ptr.moving && mouseInf > 0;
			return {
				dt: f.dt,
				emitX: epx * N,
				emitY: (1 - epy) * N,
				emitVelX: Math.sin(dirRad) * velMag,
				emitVelY: -Math.cos(dirRad) * velMag,
				perpDirX: Math.cos(dirRad),
				perpDirY: Math.sin(dirRad),
				spreadFactor: Math.tan(spreadHalf),
				emitRad: f.num("emitRadius", .06) * N,
				emitIntensity: f.num("intensity", .7),
				dyeFade: f.num("dissipation", 1),
				velFade: VELOCITY_DISSIPATION,
				curlStrength: f.num("detail", 30),
				gravity: f.num("gravity", .5),
				colorDecay: f.num("colorDecay", 1),
				cursorX: active ? ptr.x * N : 0,
				cursorY: active ? ptr.y * N : 0,
				cursorVelX: active ? ptr.dx * N * 15 * mouseInf : 0,
				cursorVelY: active ? ptr.dy * N * 15 * mouseInf : 0,
				mouseActive: active ? 1 : 0,
				mouseRadSq: mouseRadGrid * mouseRadGrid
			};
		}
	})),
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
var Smoke_default = componentDefinition;
export { copyDensKernel as a, divergenceKernel as c, outputKernel as d, splatKernel as f, componentDefinition as i, gradSubtractKernel as l, advectDensKernel as n, copyVelKernel as o, vorticityKernel as p, advectVelKernel as r, curlKernel as s, Smoke_default as t, jacobiKernel as u };
