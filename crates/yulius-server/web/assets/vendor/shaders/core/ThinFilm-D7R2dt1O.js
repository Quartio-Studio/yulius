import { $f as f32, Ac as defineStd, Bf as sqrt, Cd as cosinePalette, Cf as select, Df as cos, Ef as clamp, Ff as mix, Hd as call, Hf as vec2f, Kd as floatE, Lf as pow, Mf as fract, Nf as max, Rf as sin, Ru as shapeEffectBoundingBoxDeclaration, Uf as vec3f, Wf as vec4f, bu as offsetUV, cf as mixColorsVariants, hi as shapedSurface, jf as floor, np as __export, qd as mixExpr, sf as mixColorsLinear, vu as glassGradient, xf as typegpu_default, xu as sdfSpaceUV } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var thinFilm_exports = /* @__PURE__ */ __export({
	applyThinFilmEffect: () => applyThinFilmEffect$1,
	thinFilmCompose: () => thinFilmCompose,
	thinFilmCustomParams: () => thinFilmCustomParams,
	thinFilmRainbow: () => thinFilmRainbow,
	thinFilmShade: () => thinFilmShade
});
var DEG_TO_RAD = .017453292519943295;
var TWO_PI = 6.283185307179586;
const thinFilmShade = typegpu_default.fn([
	f32,
	f32,
	f32,
	f32,
	f32,
	f32,
	f32,
	f32,
	f32,
	f32,
	f32,
	f32
], vec2f)(/* @__PURE__ */ (($) => (globalThis.__TYPEGPU_META__ ??= /* @__PURE__ */ new WeakMap()).set($.f = ((sdfRaw, scale, gradX, gradY, edgeSoftness, rimWidthUniform, thickness, dispersion, hueShift, lightAngle, animTime, volumetric) => {
	"use gpu";
	const sdf = __tsover_div(sdfRaw, scale);
	const sharp = max(__tsover_mul(edgeSoftness, .5), .001);
	const gLen = max(sqrt(__tsover_add(__tsover_mul(gradX, gradX), __tsover_mul(gradY, gradY))), 1e-4);
	const nx = __tsover_div(gradX, gLen);
	const ny = __tsover_div(gradY, gLen);
	const rb1 = clamp(__tsover_mul(__tsover_div(__tsover_mul(sdf, -1), sharp), 32), 0, 1);
	const rimWidth = max(__tsover_mul(rimWidthUniform, .1), .001);
	const depth = clamp(__tsover_div(__tsover_mul(sdf, -1), rimWidth), 0, 1);
	const cosView = __tsover_div(1, sqrt(__tsover_add(__tsover_mul(gLen, gLen), 1)));
	const exponent = mix(7, 1.5, clamp(rimWidthUniform, 0, 1));
	const volRim = __tsover_mul(pow(__tsover_sub(1, cosView), exponent), rb1);
	const rim = select(__tsover_mul(__tsover_mul(__tsover_sub(1, depth), __tsover_sub(1, depth)), rb1), volRim, volumetric > .5);
	const lx = cos(__tsover_mul(lightAngle, DEG_TO_RAD));
	const ly = sin(__tsover_mul(lightAngle, DEG_TO_RAD));
	const baseCos = __tsover_add(__tsover_mul(nx, lx), __tsover_mul(ny, ly));
	const baseSin = __tsover_sub(__tsover_mul(ny, lx), __tsover_mul(nx, ly));
	const theta = __tsover_mul(animTime, TWO_PI);
	const rotatedDot = __tsover_add(__tsover_mul(baseCos, cos(theta)), __tsover_mul(baseSin, sin(theta)));
	const thickVol = __tsover_mul(__tsover_mul(thickness, max(__tsover_mul(sdf, -1), 0)), 10);
	const thicknessTerm = select(__tsover_mul(__tsover_mul(thickness, depth), 2), thickVol, volumetric > .5);
	return vec2f(__tsover_add(__tsover_add(hueShift, __tsover_mul(dispersion, rotatedDot)), thicknessTerm), rim);
}), {
	v: 2,
	name: void 0,
	ast: {
		"params": [
			{
				"type": "i",
				"name": "sdfRaw"
			},
			{
				"type": "i",
				"name": "scale"
			},
			{
				"type": "i",
				"name": "gradX"
			},
			{
				"type": "i",
				"name": "gradY"
			},
			{
				"type": "i",
				"name": "edgeSoftness"
			},
			{
				"type": "i",
				"name": "rimWidthUniform"
			},
			{
				"type": "i",
				"name": "thickness"
			},
			{
				"type": "i",
				"name": "dispersion"
			},
			{
				"type": "i",
				"name": "hueShift"
			},
			{
				"type": "i",
				"name": "lightAngle"
			},
			{
				"type": "i",
				"name": "animTime"
			},
			{
				"type": "i",
				"name": "volumetric"
			}
		],
		"body": [0, [
			[
				13,
				"sdf",
				[
					1,
					"sdfRaw",
					"/",
					"scale"
				]
			],
			[
				13,
				"sharp",
				[
					6,
					"std.max",
					[[
						1,
						"edgeSoftness",
						"*",
						[5, "0.5"]
					], [5, "0.001"]]
				]
			],
			[
				13,
				"gLen",
				[
					6,
					"std.max",
					[[
						6,
						"std.sqrt",
						[[
							1,
							[
								1,
								"gradX",
								"*",
								"gradX"
							],
							"+",
							[
								1,
								"gradY",
								"*",
								"gradY"
							]
						]]
					], [5, "0.0001"]]
				]
			],
			[
				13,
				"nx",
				[
					1,
					"gradX",
					"/",
					"gLen"
				]
			],
			[
				13,
				"ny",
				[
					1,
					"gradY",
					"/",
					"gLen"
				]
			],
			[
				13,
				"rb1",
				[
					6,
					"std.clamp",
					[
						[
							1,
							[
								1,
								[
									1,
									"sdf",
									"*",
									[
										4,
										"-",
										[5, "1"]
									]
								],
								"/",
								"sharp"
							],
							"*",
							[5, "32"]
						],
						[5, "0"],
						[5, "1"]
					]
				]
			],
			[
				13,
				"rimWidth",
				[
					6,
					"std.max",
					[[
						1,
						"rimWidthUniform",
						"*",
						[5, "0.1"]
					], [5, "0.001"]]
				]
			],
			[
				13,
				"depth",
				[
					6,
					"std.clamp",
					[
						[
							1,
							[
								1,
								"sdf",
								"*",
								[
									4,
									"-",
									[5, "1"]
								]
							],
							"/",
							"rimWidth"
						],
						[5, "0"],
						[5, "1"]
					]
				]
			],
			[
				13,
				"cosView",
				[
					1,
					[5, "1"],
					"/",
					[
						6,
						"std.sqrt",
						[[
							1,
							[
								1,
								"gLen",
								"*",
								"gLen"
							],
							"+",
							[5, "1"]
						]]
					]
				]
			],
			[
				13,
				"exponent",
				[
					6,
					"std.mix",
					[
						[5, "7"],
						[5, "1.5"],
						[
							6,
							"std.clamp",
							[
								"rimWidthUniform",
								[5, "0"],
								[5, "1"]
							]
						]
					]
				]
			],
			[
				13,
				"volRim",
				[
					1,
					[
						6,
						"std.pow",
						[[
							1,
							[5, "1"],
							"-",
							"cosView"
						], "exponent"]
					],
					"*",
					"rb1"
				]
			],
			[
				13,
				"flatRim",
				[
					1,
					[
						1,
						[
							1,
							[5, "1"],
							"-",
							"depth"
						],
						"*",
						[
							1,
							[5, "1"],
							"-",
							"depth"
						]
					],
					"*",
					"rb1"
				]
			],
			[
				13,
				"rim",
				[
					6,
					"std.select",
					[
						"flatRim",
						"volRim",
						[
							1,
							"volumetric",
							">",
							[5, "0.5"]
						]
					]
				]
			],
			[
				13,
				"lx",
				[
					6,
					"std.cos",
					[[
						1,
						"lightAngle",
						"*",
						"DEG_TO_RAD"
					]]
				]
			],
			[
				13,
				"ly",
				[
					6,
					"std.sin",
					[[
						1,
						"lightAngle",
						"*",
						"DEG_TO_RAD"
					]]
				]
			],
			[
				13,
				"baseCos",
				[
					1,
					[
						1,
						"nx",
						"*",
						"lx"
					],
					"+",
					[
						1,
						"ny",
						"*",
						"ly"
					]
				]
			],
			[
				13,
				"baseSin",
				[
					1,
					[
						1,
						"ny",
						"*",
						"lx"
					],
					"-",
					[
						1,
						"nx",
						"*",
						"ly"
					]
				]
			],
			[
				13,
				"theta",
				[
					1,
					"animTime",
					"*",
					"TWO_PI"
				]
			],
			[
				13,
				"rotatedDot",
				[
					1,
					[
						1,
						"baseCos",
						"*",
						[
							6,
							"std.cos",
							["theta"]
						]
					],
					"+",
					[
						1,
						"baseSin",
						"*",
						[
							6,
							"std.sin",
							["theta"]
						]
					]
				]
			],
			[
				13,
				"thickVol",
				[
					1,
					[
						1,
						"thickness",
						"*",
						[
							6,
							"std.max",
							[[
								1,
								"sdf",
								"*",
								[
									4,
									"-",
									[5, "1"]
								]
							], [5, "0"]]
						]
					],
					"*",
					[5, "10"]
				]
			],
			[
				13,
				"thickFlat",
				[
					1,
					[
						1,
						"thickness",
						"*",
						"depth"
					],
					"*",
					[5, "2"]
				]
			],
			[
				13,
				"thicknessTerm",
				[
					6,
					"std.select",
					[
						"thickFlat",
						"thickVol",
						[
							1,
							"volumetric",
							">",
							[5, "0.5"]
						]
					]
				]
			],
			[
				13,
				"phase",
				[
					1,
					[
						1,
						"hueShift",
						"+",
						[
							1,
							"dispersion",
							"*",
							"rotatedDot"
						]
					],
					"+",
					"thicknessTerm"
				]
			],
			[10, [
				6,
				"d.vec2f",
				["phase", "rim"]
			]]
		]]
	},
	externals: {
		"std.max": () => max,
		"std.sqrt": () => sqrt,
		"std.clamp": () => clamp,
		"std.mix": () => mix,
		"std.pow": () => pow,
		"std.select": () => select,
		"std.cos": () => cos,
		"DEG_TO_RAD": () => DEG_TO_RAD,
		"std.sin": () => sin,
		"TWO_PI": () => TWO_PI,
		"d.vec2f": () => vec2f
	}
}) && $.f)({}));
const thinFilmRainbow = typegpu_default.fn([f32], vec3f)(/* @__PURE__ */ (($) => (globalThis.__TYPEGPU_META__ ??= /* @__PURE__ */ new WeakMap()).set($.f = ((phase) => {
	"use gpu";
	return cosinePalette(phase, 0, .3333, .6667);
}), {
	v: 2,
	name: void 0,
	ast: {
		"params": [{
			"type": "i",
			"name": "phase"
		}],
		"body": [0, [[10, [
			6,
			"cosinePalette",
			[
				"phase",
				[5, "0"],
				[5, "0.3333"],
				[5, "0.6667"]
			]
		]]]]
	},
	externals: { "cosinePalette": () => cosinePalette }
}) && $.f)({}));
const thinFilmCustomParams = typegpu_default.fn([f32], vec3f)(/* @__PURE__ */ (($) => (globalThis.__TYPEGPU_META__ ??= /* @__PURE__ */ new WeakMap()).set($.f = ((phase) => {
	"use gpu";
	const t = fract(phase);
	const t3 = __tsover_mul(t, 3);
	const si = fract(t3);
	const floorT3 = floor(t3);
	return vec3f(si, select(f32(0), f32(1), floorT3 <= 1.5), select(f32(0), f32(1), floorT3 <= .5));
}), {
	v: 2,
	name: void 0,
	ast: {
		"params": [{
			"type": "i",
			"name": "phase"
		}],
		"body": [0, [
			[
				13,
				"t",
				[
					6,
					"std.fract",
					["phase"]
				]
			],
			[
				13,
				"t3",
				[
					1,
					"t",
					"*",
					[5, "3"]
				]
			],
			[
				13,
				"si",
				[
					6,
					"std.fract",
					["t3"]
				]
			],
			[
				13,
				"floorT3",
				[
					6,
					"std.floor",
					["t3"]
				]
			],
			[
				13,
				"gate12",
				[
					6,
					"std.select",
					[
						[
							6,
							"d.f32",
							[[5, "0"]]
						],
						[
							6,
							"d.f32",
							[[5, "1"]]
						],
						[
							1,
							"floorT3",
							"<=",
							[5, "1.5"]
						]
					]
				]
			],
			[
				13,
				"gate01",
				[
					6,
					"std.select",
					[
						[
							6,
							"d.f32",
							[[5, "0"]]
						],
						[
							6,
							"d.f32",
							[[5, "1"]]
						],
						[
							1,
							"floorT3",
							"<=",
							[5, "0.5"]
						]
					]
				]
			],
			[10, [
				6,
				"d.vec3f",
				[
					"si",
					"gate12",
					"gate01"
				]
			]]
		]]
	},
	externals: {
		"std.fract": () => fract,
		"std.floor": () => floor,
		"std.select": () => select,
		"d.f32": () => f32,
		"d.vec3f": () => vec3f
	}
}) && $.f)({}));
const thinFilmCompose = typegpu_default.fn([
	vec3f,
	f32,
	f32,
	f32
], vec4f)(/* @__PURE__ */ (($) => (globalThis.__TYPEGPU_META__ ??= /* @__PURE__ */ new WeakMap()).set($.f = ((baseColor, rim, intensity, saturation) => {
	"use gpu";
	const rgb = mix(vec3f(1, 1, 1), baseColor, saturation).mul(rim).mul(intensity);
	const alpha = clamp(rim, 0, 1);
	return vec4f(rgb.x, rgb.y, rgb.z, alpha);
}), {
	v: 2,
	name: void 0,
	ast: {
		"params": [
			{
				"type": "i",
				"name": "baseColor"
			},
			{
				"type": "i",
				"name": "rim"
			},
			{
				"type": "i",
				"name": "intensity"
			},
			{
				"type": "i",
				"name": "saturation"
			}
		],
		"body": [0, [
			[
				13,
				"white",
				[
					6,
					"d.vec3f",
					[
						[5, "1"],
						[5, "1"],
						[5, "1"]
					]
				]
			],
			[
				13,
				"col",
				[
					6,
					"std.mix",
					[
						"white",
						"baseColor",
						"saturation"
					]
				]
			],
			[
				13,
				"rgb",
				[
					6,
					[
						7,
						[
							6,
							[
								7,
								"col",
								"mul"
							],
							["rim"]
						],
						"mul"
					],
					["intensity"]
				]
			],
			[
				13,
				"alpha",
				[
					6,
					"std.clamp",
					[
						"rim",
						[5, "0"],
						[5, "1"]
					]
				]
			],
			[10, [
				6,
				"d.vec4f",
				[
					[
						7,
						"rgb",
						"x"
					],
					[
						7,
						"rgb",
						"y"
					],
					[
						7,
						"rgb",
						"z"
					],
					"alpha"
				]
			]]
		]]
	},
	externals: {
		"d.vec3f": () => vec3f,
		"std.mix": () => mix,
		"std.clamp": () => clamp,
		"d.vec4f": () => vec4f
	}
}) && $.f)({}));
function applyThinFilmEffect$1(params, sdfSampler, options) {
	const { uniforms, propValues, ctx } = params;
	const volumetric = options?.volumetric ?? false;
	const gradSampler = options?.gradSampler ?? sdfSampler;
	const volFlag = floatE(volumetric ? 1 : 0);
	const animTime = params.props.member("_animTime");
	const sdfUV = call(sdfSpaceUV, "sdfSpaceUV", [
		uniforms.center,
		uniforms.scale,
		uniforms.rotation,
		ctx.uv,
		ctx.aspect
	]);
	const surf0 = sdfSampler(sdfUV);
	const sdfRaw = surf0.member("r");
	let gradX;
	let gradY;
	if (options?.bakedGradients) {
		gradX = surf0.member("g");
		gradY = surf0.member("b");
	} else {
		const surfX = gradSampler(call(offsetUV, "offsetUV", [
			sdfUV,
			floatE(.01),
			floatE(0)
		]));
		const surfY = gradSampler(call(offsetUV, "offsetUV", [
			sdfUV,
			floatE(0),
			floatE(.01)
		]));
		const grad = call(glassGradient, "glassGradient", [
			sdfRaw,
			surfX.member("r"),
			surfY.member("r")
		]);
		gradX = grad.member("x");
		gradY = grad.member("y");
	}
	const shade = call(thinFilmShade, "thinFilmShade", [
		sdfRaw,
		uniforms.scale,
		gradX,
		gradY,
		uniforms.edgeSoftness,
		uniforms.rimWidth,
		uniforms.thickness,
		uniforms.dispersion,
		uniforms.hueShift,
		uniforms.lightAngle,
		animTime,
		volFlag
	]);
	const phase = shade.member("x");
	const rim = shade.member("y");
	let baseColor;
	if (propValues.mode === "custom") {
		const mixFn = mixColorsVariants[typeof propValues.colorSpace === "number" ? propValues.colorSpace : 0] ?? mixColorsLinear;
		const cp = call(thinFilmCustomParams, "thinFilmCustomParams", [phase]);
		const si = cp.member("x");
		const gate12 = cp.member("y");
		const gate01 = cp.member("z");
		const c01 = call(mixFn, "mixColors", [
			uniforms.colorA,
			uniforms.colorB,
			si
		]);
		const c12 = call(mixFn, "mixColors", [
			uniforms.colorB,
			uniforms.colorC,
			si
		]);
		baseColor = mixExpr(mixExpr(call(mixFn, "mixColors", [
			uniforms.colorC,
			uniforms.colorA,
			si
		]), c12, gate12), c01, gate01).member("rgb");
	} else baseColor = call(thinFilmRainbow, "thinFilmRainbow", [phase]);
	return call(thinFilmCompose, "thinFilmCompose", [
		baseColor,
		rim,
		uniforms.intensity,
		uniforms.saturation
	]);
}
var { applyThinFilmEffect } = thinFilm_exports;
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
const componentDefinition = defineStd({
	name: "ThinFilm",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "Iridescent thin-film edge",
	animatedTime: { speed: "speed" },
	...shapedSurface({
		chord: "firstLobe",
		gradSampler: "same",
		bakedGradients: "volumetric",
		stencil: "none",
		surface: (frame, params) => applyThinFilmEffect(params, frame.sampler, {
			volumetric: frame.volumetric,
			bakedGradients: frame.bakedGradients
		})
	}),
	props: {
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
				type: ["range", "map"],
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
		intensity: {
			default: 1,
			description: "Rim brightness — values above 1 push the edge past white for HDR bloom",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 4,
				step: .01,
				label: "Intensity",
				group: "Film"
			}
		},
		rimWidth: {
			default: 1,
			description: "How far inward from the edge the iridescence reaches",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 3,
				step: .01,
				label: "Rim Width",
				group: "Film"
			}
		},
		edgeSoftness: {
			default: .3,
			description: "Softness of the shape boundary",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Edge Softness",
				group: "Film"
			}
		},
		thickness: {
			default: .5,
			description: "Film thickness — higher values pack more spectral bands across the rim",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Thickness",
				group: "Film"
			}
		},
		dispersion: {
			default: .5,
			description: "How strongly color separates around the perimeter (warm vs cool sides)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Dispersion",
				group: "Film"
			}
		},
		saturation: {
			default: 1,
			description: "Spectral saturation — 0 = white rim, 1 = full iridescence",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Saturation",
				group: "Film"
			}
		},
		hueShift: {
			default: 0,
			description: "Rotate the spectrum to reposition the colors",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Hue Shift",
				group: "Film"
			}
		},
		lightAngle: {
			default: 300,
			description: "Light angle in degrees — sets which side of the rim runs warm vs cool",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Light Angle",
				group: "Film"
			}
		},
		mode: {
			default: "rainbow",
			compileTime: true,
			description: "Rainbow uses the full iridescent spectrum; Custom cycles through your three chosen colors",
			ui: {
				type: "select",
				options: [{
					label: "Rainbow",
					value: "rainbow"
				}, {
					label: "Custom",
					value: "custom"
				}],
				label: "Mode",
				group: "Colors"
			}
		},
		colorA: {
			default: "#2b6fff",
			transform: transformColor,
			description: "First color in the rim cycle",
			ui: {
				type: "color",
				label: "Color 1",
				group: "Colors",
				condition: { mode: "custom" }
			}
		},
		colorB: {
			default: "#ffffff",
			transform: transformColor,
			description: "Second color in the rim cycle",
			ui: {
				type: "color",
				label: "Color 2",
				group: "Colors",
				condition: { mode: "custom" }
			}
		},
		colorC: {
			default: "#ff7a21",
			transform: transformColor,
			description: "Third color in the rim cycle",
			ui: {
				type: "color",
				label: "Color 3",
				group: "Colors",
				condition: { mode: "custom" }
			}
		},
		colorSpace: {
			default: "oklch",
			transform: transformColorSpace,
			compileTime: true,
			description: "color space used to blend between custom colors",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors",
				condition: { mode: "custom" }
			}
		},
		speed: {
			default: .1,
			description: "Speed at which the colors rotate around the rim (0 = static)",
			ui: {
				type: "range",
				min: -1,
				max: 1,
				step: .01,
				label: "Speed",
				group: "Animation"
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
			description: "URL to a pre-generated SDF .bin file — when non-empty, activates SVG mode and triggers a shader recompile"
		},
		shapeType: {
			default: "",
			compileTime: true,
			description: "Active SDF shape type — triggers recompile when shape is switched. When empty, derived from shape JSON at mount time."
		}
	}
});
var ThinFilm_default = componentDefinition;
export { componentDefinition as n, ThinFilm_default as t };
