import { $f as f32, Ac as defineStd, Bd as ZERO, Gs as clamp, Hd as call, Jd as vec4, Jf as textureStorage2d, Lu as resolveShapeType, Nu as sdf3d_exports, Ru as shapeEffectBoundingBoxDeclaration, Wf as vec4f, Xf as struct, Yf as arrayOf, af as colorMixing_exports, dc as mul, f as fluidSim, h as splat, ll as buildEmitterSplatKernel, pl as buildStableFluidsKernels, rd as sdf_exports, ul as buildFluidOutputKernel, vl as makeShapeMaskSet, xf as typegpu_default, yu as glass_exports } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var { sdfSpaceUV } = glass_exports;
var { createSvgSdfSampler, createSdfDataTexture, createAnalyticSdfSampler, driveAnalyticSubProps, ANALYTIC_SDF_EXTRA_FIELDS } = sdf_exports;
var { createVolumetricFieldComputeNode, buildVolumetricFieldSampler, VOLUMETRIC_FIELD_EXTRA_FIELDS } = sdf3d_exports;
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
var N = 256;
var COUNT = N * N;
var JACOBI_ITERS = 10;
var VELOCITY_DISSIPATION = .2;
var STATE_FORMAT = "rgba16float";
var cm = colorMixing_exports;
var FillParams = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(struct({
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
	colorDecay: f32,
	centerX: f32,
	centerY: f32,
	scale: f32,
	rotation: f32,
	aspect: f32,
	cursorX: f32,
	cursorY: f32,
	cursorVelX: f32,
	cursorVelY: f32,
	mouseActive: f32,
	mouseRadSq: f32,
	saRadius: f32,
	saSides: f32,
	saRounding: f32,
	saInnerRatio: f32,
	saRotation: f32,
	saHeight: f32,
	saOffset: f32,
	saAperture: f32,
	vfOriginX: f32,
	vfOriginY: f32,
	vfSpanX: f32,
	vfSpanY: f32,
	vfActiveRes: f32
}), "FillParams");
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
	maskBuf: {
		storage: arrayOf(vec4f, COUNT),
		access: "readonly"
	},
	params: { uniform: FillParams }
}), "fluidLayout");
var outputLayout = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(typegpu_default.bindGroupLayout({
	dyeA: {
		storage: arrayOf(vec4f, COUNT),
		access: "readonly"
	},
	outTex: { storageTexture: textureStorage2d(STATE_FORMAT, "write-only") }
}), "outputLayout");
var maskSet = makeShapeMaskSet(FillParams, {
	n: N,
	namePrefix: "smokeFill"
});
const makeMaskKernel = maskSet.analytic;
const svgMaskKernel = maskSet.svgKernel;
const vfMaskKernel = maskSet.fieldKernel;
const splatKernel = buildEmitterSplatKernel(fluidLayout, {
	n: N,
	namePrefix: "smokeFill",
	cone: true,
	cursorPush: true,
	masked: true,
	densityGain: 8,
	velocityBlendGain: 3
});
var solverKernels = buildStableFluidsKernels(fluidLayout, {
	n: N,
	namePrefix: "smokeFill",
	boundary: "clamped",
	dye: "densityAge",
	dyeDissipation: true,
	ageAdvance: true,
	solidMask: true
});
const { curl: curlKernel, vorticity: vorticityKernel, divergence: divergenceKernel, jacobi: jacobiKernel, gradSubtract: gradSubtractKernel, advectVel: advectVelKernel, copyVel: copyVelKernel, advectDye: advectDensKernel, copyDye: copyDensKernel } = solverKernels;
const outputKernel = buildFluidOutputKernel(outputLayout, {
	n: N,
	namePrefix: "smokeFill",
	dye: "densityAge"
});
function shapeContainerField(params) {
	const { getCpuValue } = params;
	const shapeSdfUrl = getCpuValue("shapeSdfUrl") || "";
	const shapeType = resolveShapeType(getCpuValue("shapeType") || "", getCpuValue("shape"));
	const vf = createVolumetricFieldComputeNode(params, () => getCpuValue("shape"), "none");
	return {
		setup(ctx) {
			const { root } = ctx;
			const maskBuf = ctx.buffers.maskBuf;
			const paramsBuffer = ctx.paramsBuffer;
			let maskPass;
			if (vf) {
				const maskVfBg = root.createBindGroup(maskSet.fieldLayout, {
					maskBuf,
					params: paramsBuffer,
					fieldTex: vf.fieldTexture
				});
				maskPass = ctx.pass(vfMaskKernel, maskVfBg);
			} else if (shapeSdfUrl) {
				const sdfTex = createSdfDataTexture(ctx.params, shapeSdfUrl);
				const maskSvgBg = root.createBindGroup(maskSet.svgLayout, {
					maskBuf,
					params: paramsBuffer,
					sdfSource: sdfTex.texture.texture
				});
				maskPass = ctx.pass(svgMaskKernel, maskSvgBg);
			} else {
				const maskBg = root.createBindGroup(maskSet.analyticLayout, {
					maskBuf,
					params: paramsBuffer
				});
				maskPass = ctx.pass(makeMaskKernel(shapeType), maskBg);
			}
			return {
				maskPass,
				outputs: vf ? vf.outputs : {},
				preFrame: (fp) => vf?.getComputeNodes(fp) ?? null
			};
		},
		domain: () => vf?.getFieldDomain()
	};
}
function makeSubPropResolver(getCpuValue) {
	const num = (v, f) => typeof v === "number" ? v : f;
	return () => {
		const raw = getCpuValue("shape");
		let c = {};
		if (raw && typeof raw === "object") c = raw;
		else if (typeof raw === "string") try {
			c = JSON.parse(raw);
		} catch {
			c = {};
		}
		return {
			saRadius: num(c.radius, num(c.width, num(c.bottomWidth, .35))),
			saSides: num(c.sides, 6),
			saRounding: num(c.rounding, 0),
			saInnerRatio: num(c.innerRatio, num(c.thickness, num(c.spread, num(c.topWidth, num(c.topRatio, .4))))),
			saRotation: num(c.rotation, 0),
			saHeight: num(c.height, .25),
			saOffset: num(c.offset, num(c.skew, .2)),
			saAperture: num(c.aperture, 270)
		};
	};
}
const componentDefinition = defineStd({
	name: "SmokeFill",
	role: "simulation",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "Fill a shape with swirling fluid smoke that interacts with the shape boundary",
	requiresRTT: false,
	requiresChild: false,
	usesPointer: true,
	extraFields: {
		...VOLUMETRIC_FIELD_EXTRA_FIELDS,
		...ANALYTIC_SDF_EXTRA_FIELDS
	},
	props: {
		colorA: {
			default: "#8cf3ff",
			transform: transformColor,
			description: "Color of fresh smoke",
			ui: {
				type: "color",
				label: "Start Color",
				group: "Colors"
			}
		},
		colorB: {
			default: "#04a0d6",
			transform: transformColor,
			description: "Color smoke transitions to as it ages",
			ui: {
				type: "color",
				label: "End Color",
				group: "Colors"
			}
		},
		origin: {
			default: "center",
			description: "Reference edge the center position is measured from (center default). Lets you pin the shape relative to a corner or the canvas centre.",
			ui: {
				type: "origin",
				label: "Origin",
				group: "Position"
			}
		},
		center: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "Center position of the shape",
			ui: {
				type: "position",
				label: "Center",
				group: "Position",
				units: ["%", "px"]
			}
		},
		scale: {
			default: 1,
			description: "Scale of the shape (1 = default size)",
			ui: {
				type: "range",
				min: .1,
				max: 3,
				step: .01,
				label: "Scale",
				group: "Position"
			}
		},
		rotation: {
			default: 0,
			description: "Rotation of the shape in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Position"
			}
		},
		emitFrom: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "Emission source point within the shape",
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
			default: 10,
			description: "Emission velocity strength",
			ui: {
				type: "range",
				min: .1,
				max: 30,
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
			default: .03,
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
			default: .3,
			description: "How fast smoke fades over time",
			ui: {
				type: "range",
				min: .1,
				max: 5,
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
			description: "Downward gravitational pull on smoke — 0 = weightless, negative values = smoke rises",
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
		},
		shape: {
			default: DEFAULT_SHAPE_CONFIG,
			description: "Serialized shape configuration (JSON)",
			ui: {
				type: "shape",
				label: "Shape",
				group: "Shape"
			}
		},
		shapeSdfUrl: {
			default: "",
			compileTime: true,
			description: "URL to a pre-generated SDF .bin file"
		},
		shapeType: {
			default: "",
			compileTime: true,
			description: "Active SDF shape type"
		}
	},
	...fluidSim((params) => {
		const { getCpuValue } = params;
		const container = shapeContainerField(params);
		const resolveSub = makeSubPropResolver(getCpuValue);
		return {
			resolution: N,
			layout: fluidLayout,
			outputLayout,
			paramsSchema: FillParams,
			solver: {
				kernels: solverKernels,
				jacobiIters: JACOBI_ITERS
			},
			output: {
				kernel: outputKernel,
				key: "smokeTexture"
			},
			pointer: { minDrag: 5e-4 },
			container,
			inject: [splat({ kernel: splatKernel })],
			values: (f) => {
				const speed = f.num("speed", 2);
				const dirRad = f.num("direction", 0) * Math.PI / 180;
				const spreadHalf = Math.min(89 * Math.PI / 180, f.num("spread", 60) * Math.PI / 360);
				const centerView = getCpuValue("center");
				const emitView = getCpuValue("emitFrom");
				const h = f.frameParams.dimensions?.height ?? 0;
				const aspect = h > 0 ? (f.frameParams.dimensions?.width ?? 0) / h : 1;
				const velMag = speed * N * .15;
				const sub = resolveSub();
				const mouseInf = f.num("mouseInfluence", .5);
				const mouseRad = f.num("mouseRadius", .12);
				const ptr = f.ptr;
				const active = ptr.moving && mouseInf > 0;
				const mouseRadGrid = mouseRad * N;
				const dom = container.domain();
				return {
					dt: f.dt,
					emitX: (emitView?.x ?? .5) * N,
					emitY: (1 - (emitView?.y ?? .5)) * N,
					emitVelX: Math.sin(dirRad) * velMag,
					emitVelY: -Math.cos(dirRad) * velMag,
					perpDirX: Math.cos(dirRad),
					perpDirY: Math.sin(dirRad),
					spreadFactor: Math.tan(spreadHalf),
					emitRad: f.num("emitRadius", .03) * N,
					emitIntensity: f.num("intensity", .8),
					dyeFade: f.num("dissipation", .5),
					velFade: VELOCITY_DISSIPATION,
					curlStrength: f.num("detail", 30),
					gravity: f.num("gravity", .5),
					colorDecay: f.num("colorDecay", 1),
					centerX: centerView?.x ?? .5,
					centerY: 1 - (centerView?.y ?? .5),
					scale: f.num("scale", 1),
					rotation: f.num("rotation", 0) * Math.PI / 180,
					aspect,
					cursorX: active ? ptr.x * N : 0,
					cursorY: active ? ptr.y * N : 0,
					cursorVelX: active ? ptr.dx * N * 15 * mouseInf : 0,
					cursorVelY: active ? ptr.dy * N * 15 * mouseInf : 0,
					mouseActive: active ? 1 : 0,
					mouseRadSq: mouseRadGrid * mouseRadGrid,
					...sub,
					vfOriginX: dom?.originX ?? 0,
					vfOriginY: dom?.originY ?? 0,
					vfSpanX: dom?.spanX ?? 1,
					vfSpanY: dom?.spanY ?? 1,
					vfActiveRes: dom?.activeRes ?? 1
				};
			}
		};
	}),
	gpu: { fragment: (params) => {
		const { ctx, computeOutputs, propValues, uniforms, getCpuValue } = params;
		const smokeField = computeOutputs?.smokeTexture;
		if (!smokeField) return ZERO;
		let sampler;
		const fieldTex = computeOutputs?.volumetricFieldTexture;
		if (fieldTex) sampler = buildVolumetricFieldSampler(fieldTex, {
			originX: uniforms._vfOriginX,
			originY: uniforms._vfOriginY,
			spanX: uniforms._vfSpanX,
			spanY: uniforms._vfSpanY,
			activeResF: uniforms._vfActiveRes,
			rBound: uniforms._vfRBound
		}).volumetricFieldSampler;
		else {
			const shapeSdfUrl = propValues.shapeSdfUrl || "";
			if (shapeSdfUrl) sampler = createSvgSdfSampler(params, shapeSdfUrl);
			else sampler = createAnalyticSdfSampler(resolveShapeType(propValues.shapeType || "", getCpuValue("shape")), driveAnalyticSubProps(params, () => getCpuValue("shape")));
		}
		const sdfUV = call(sdfSpaceUV, "sdfSpaceUV", [
			uniforms.center,
			uniforms.scale,
			uniforms.rotation,
			ctx.uv,
			ctx.aspect
		]);
		const shapeMask = clamp(mul(sampler(sdfUV).member("x"), -(N * 4)), 0, 1);
		const s = smokeField.sample(ctx.uv, "linearClamp");
		const smokeDensity = s.member("x");
		const smokeAge = s.member("y");
		const colorSpaceMode = propValues.colorSpace ?? 0;
		const smokeColor = call(cm.mixColorsVariants[colorSpaceMode] ?? cm.mixColorsLinear, "mixColors", [
			uniforms.colorA,
			uniforms.colorB,
			smokeAge
		]);
		return vec4(smokeColor.member("rgb"), smokeColor.member("a").mul(smokeDensity).mul(shapeMask));
	} }
});
var SmokeFill_default = componentDefinition;
export { copyDensKernel as a, divergenceKernel as c, makeMaskKernel as d, outputKernel as f, vorticityKernel as g, vfMaskKernel as h, componentDefinition as i, gradSubtractKernel as l, svgMaskKernel as m, advectDensKernel as n, copyVelKernel as o, splatKernel as p, advectVelKernel as r, curlKernel as s, SmokeFill_default as t, jacobiKernel as u };
