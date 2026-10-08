import { $s as fract, Ac as defineStd, Dc as vec3, Ec as vec2, Ed as mixColorStopsRuntime, Gs as clamp, Hd as call, Ic as animatedTime, Js as dot, Oc as vec4, Ru as shapeEffectBoundingBoxDeclaration, Tc as sub, Us as abs, Ws as add, Xr as hashNoise, Ys as exp, Zs as float, bc as smoothstep, cc as max, cf as mixColorsVariants, dc as mul, hi as shapedSurface, ki as valueNoise, lc as min, mc as pow, oc as local, or as directionFrame, ou as colorStopsPropConfig, qs as div, sf as mixColorsLinear, uc as mix } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
var DEFAULT_STOPS = [
	{
		color: "#02010f",
		position: 0
	},
	{
		color: "#2a0a8a",
		position: .2
	},
	{
		color: "#a41c9b",
		position: .45
	},
	{
		color: "#e8632b",
		position: .7
	},
	{
		color: "#f9e25f",
		position: .9
	},
	{
		color: "#ffffff",
		position: 1
	}
];
var heatmapShadow = (a, b, tk, phase) => {
	const posS = mix(-.6, .6, tk);
	const wob = sub(valueNoise(add(mul(b, 4), mul(float(phase), 13.7)), add(mul(tk, 3), mul(float(phase), 7.1))), .5);
	const aEff = local(add(sub(a, posS), mul(wob, .12)), "aEff");
	return clamp(mul(exp(div(mul(mul(aEff, aEff), -1), .0256)), mul(smoothstep(0, .15, tk), sub(1, smoothstep(.85, 1, tk)))), 0, 1);
};
const componentDefinition = defineStd({
	name: "Heatmap",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "Thermal-camera heat flowing through any 2D, SVG, or 3D shape. Powered by Paper Shaders.",
	animatedTime: { speed: "speed" },
	...shapedSurface({
		stencil: "centre",
		surface: (frame, params) => {
			const { uniforms, propValues } = params;
			const t = animatedTime(params);
			const suv = local(frame.sdfUV, "suv");
			const scl = max(uniforms.scale, .001);
			const f = local(div(frame.surf0.member("r"), scl), "f");
			const inside = local(sub(1, smoothstep(-.003, .003, f)), "inside");
			const travel = directionFrame(uniforms.angle, "travel");
			const rel = local(sub(suv.member("xy"), vec2(.5, .5)), "rel");
			const a = local(dot(rel, travel.tangent), "a");
			const b = local(mul(dot(rel, travel.perp), -1), "b");
			const tBase = local(sub(mul(.1, t), .3), "tBase");
			let inner = add(.8, mul(.8, exp(div(mul(max(mul(f, -1), 0), -1), .12))));
			inner = local(mix(inner, 0, heatmapShadow(a, b, fract(tBase), 1)), "inner1");
			inner = local(mix(inner, 0, heatmapShadow(a, b, fract(add(tBase, 1 / 3)), 2)), "inner2");
			inner = local(mix(inner, 0, heatmapShadow(a, b, fract(add(tBase, 2 / 3)), 3)), "inner3");
			inner = mul(inner, mul(2, uniforms.innerGlow));
			inner = add(inner, mul(mul(uniforms.contour, 2), exp(div(mul(abs(f), -1), .03))));
			inner = mul(min(inner, 1), inside);
			inner = local(pow(max(inner, 0), 1.2), "inner");
			const outerBlur = exp(div(mul(max(f, 0), -1), add(.05, mul(.15, uniforms.outerGlow))));
			const t3 = fract(sub(mul(tBase, 3), .1));
			const yy = local(fract(sub(mul(a, 1.2), t3)), "yy");
			const animatedMask = add(.5, mul(smoothstep(.3, .65, yy), sub(1, smoothstep(.65, 1, yy))));
			const outer = local(mul(mul(mul(mul(mul(mul(.9, pow(outerBlur, .8)), animatedMask), 5), uniforms.outerGlow), uniforms.outerGlow), sub(1, inside)), "outer");
			let heat = clamp(add(inner, outer), 0, 1);
			const frameFade = local(mul(mul(mul(smoothstep(-.06, .1, suv.member("x")), sub(1, smoothstep(.9, 1.06, suv.member("x")))), smoothstep(-.06, .1, suv.member("y"))), sub(1, smoothstep(.9, 1.06, suv.member("y")))), "frameFade");
			heat = mul(heat, frameFade);
			heat = add(heat, mul(.005, sub(hashNoise(mul(suv.member("x"), 991.3), mul(suv.member("y"), 787.7)), .5)));
			heat = local(clamp(heat, 0, 1), "heat");
			const coverage = mul(clamp(add(inside, outer), 0, 1), frameFade);
			const colorSpaceMode = propValues.colorSpace ?? 0;
			const stopCount = propValues.stopCount ?? 0;
			let ramp;
			if (stopCount > 1) ramp = mixColorStopsRuntime(heat, {
				colorsArray: uniforms.colorsArray,
				positionsArray: uniforms.positionsArray,
				convertedColorsArray: uniforms.convertedColorsArray,
				stopCount: uniforms.stopCount
			}, colorSpaceMode);
			else ramp = call(mixColorsVariants[colorSpaceMode] ?? mixColorsLinear, "mixColors", [
				uniforms.colorA,
				uniforms.colorB,
				heat
			]);
			const rampL = local(ramp, "ramp");
			return vec4(clamp(rampL.member("xyz"), vec3(0, 0, 0), vec3(1, 1, 1)), mul(rampL.member("w"), coverage));
		}
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
			description: "Center position of the heatmap shape",
			ui: {
				type: "position",
				label: "Center",
				group: "Position",
				units: ["%", "px"]
			}
		},
		scale: {
			default: 1,
			description: "Scale of the heatmap shape (1 = default size)",
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
			description: "Rotation of the heatmap shape in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Position"
			}
		},
		colorA: {
			default: "#02010f",
			transform: transformColor,
			description: "Cold color (used when no gradient stops are set)",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#f9e25f",
			transform: transformColor,
			description: "Hot color (used when no gradient stops are set)",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		stops: {
			...colorStopsPropConfig(),
			default: DEFAULT_STOPS
		},
		colorSpace: {
			default: "oklab",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space for the heat ramp interpolation",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		},
		innerGlow: {
			default: .4,
			description: "Heat filling the inside of the shape",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Inner Glow",
				group: "Heat"
			}
		},
		outerGlow: {
			default: .2,
			description: "Heat radiating beyond the silhouette",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Outer Glow",
				group: "Heat"
			}
		},
		contour: {
			default: .5,
			description: "Heat concentrated along the shape boundary",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Contour",
				group: "Heat"
			}
		},
		angle: {
			default: 90,
			description: "Direction the heat waves travel, in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Angle",
				group: "Heat"
			}
		},
		speed: {
			default: 1,
			description: "Speed of the flowing heat. 0 pauses.",
			ui: {
				type: "range",
				min: 0,
				max: 4,
				step: .01,
				label: "Speed",
				group: "Heat"
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
	}
});
var Heatmap_default = componentDefinition;
export { componentDefinition as n, Heatmap_default as t };
