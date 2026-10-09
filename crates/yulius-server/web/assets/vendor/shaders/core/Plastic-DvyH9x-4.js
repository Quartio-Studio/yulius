import { $r as keyLightAt, Ac as defineStd, Bd as ZERO, Cc as sqrt, Ci as surfaceNoiseAt, Dc as vec3, Ec as vec2, Ed as mixColorStopsRuntime, Gs as clamp, Hd as call, Hr as fieldSlope, Ic as animatedTime, Js as dot, Oc as vec4, Ru as shapeEffectBoundingBoxDeclaration, Sc as splat3, Si as surfaceNoise, Tc as sub, Us as abs, Ws as add, Xs as exp2, Yr as guarded, Zr as insideShape, ar as direction, bc as smoothstep, cc as max, cf as mixColorsVariants, dc as mul, hc as reflect, hi as shapedSurface, mc as pow, oc as local, ou as colorStopsPropConfig, pc as normalize, qr as grazingFlat, qs as div, ri as marchedNormal, sf as mixColorsLinear, si as nudgeNormal, uc as mix, vi as silhouette, wi as surfacePattern, xi as surfaceField } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
var envBox = (x, lo, hi, softE, hint) => {
	const s = local(softE, `${hint}S`);
	const xx = local(x, `${hint}X`);
	return mul(smoothstep(sub(lo, s), add(lo, s), xx), sub(1, smoothstep(sub(hi, s), add(hi, s), xx)));
};
var crumpleField = (px, py, cScale, seedOff) => {
	const p1 = vec2(add(mul(px, cScale), seedOff), add(mul(py, cScale), mul(seedOff, .617)));
	const p2 = vec2(add(add(mul(mul(px, cScale), 2.3), 13.1), seedOff), add(add(mul(mul(py, cScale), 2.3), 7.7), mul(seedOff, .617)));
	return add(abs(surfaceNoiseAt(p1)), mul(abs(surfaceNoiseAt(p2)), .45));
};
const componentDefinition = defineStd({
	name: "Plastic",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "Glossy molded plastic with photorealistic studio reflections, driven in a custom shape",
	animatedTime: { speed: "speed" },
	...shapedSurface({
		pattern: "raw",
		surface: (frame, params) => {
			const { uniforms: u, propValues } = params;
			const t = animatedTime(params);
			const vol = frame.volumetric;
			const field = surfaceField(frame, params, { scale: u.scale });
			const { sdf, pxH } = field;
			const sdfUV = local(frame.sdfUV, "sdfUV");
			const gDir = direction(u.gradientAngle, "gDir");
			const gT = smoothstep(0, 1, clamp(add(div(dot(sub(sdfUV.member("xy"), vec2(.5, .5)), gDir), .7), .5), 0, 1));
			const colorSpaceMode = propValues.colorSpace ?? 0;
			const stopCount = propValues.stopCount ?? 0;
			let baseColor;
			if (stopCount > 1) baseColor = mixColorStopsRuntime(gT, {
				colorsArray: u.colorsArray,
				positionsArray: u.positionsArray,
				convertedColorsArray: u.convertedColorsArray,
				stopCount: u.stopCount
			}, colorSpaceMode);
			else baseColor = call(mixColorsVariants[colorSpaceMode] ?? mixColorsLinear, "mixColors", [
				u.colorA,
				u.colorB,
				gT
			]);
			const baseRgb = baseColor.member("rgb");
			const light = direction(add(u.lightAngle, mul(t, 6)), "light");
			const lx = light.member("x");
			const ly = light.member("y");
			const grad = fieldSlope(field);
			const transition = silhouette(field, u.edgeSoftness);
			const pat = surfacePattern(frame, field, { uv: sdfUV });
			const pdx = pat.member("x");
			const pdy = pat.member("y");
			const thicknessRange = max(mul(u.thickness, .3), .01);
			const depthNorm = local(clamp(div(mul(sdf, -1), thicknessRange), 0, 1), "depthNorm");
			let nBase;
			if (vol) nBase = marchedNormal(field, 4, "nBase");
			else {
				const bevelSlope = local(mul(sqrt(max(sub(1, mul(depthNorm, depthNorm)), 0)), 2.4), "bevelSlope");
				nBase = local(normalize(vec3(mul(grad.member("x"), bevelSlope), mul(grad.member("y"), bevelSlope), -1)), "nBase");
			}
			const cScale = local(mul(u.crumpleScale, 4), "cScale");
			const cAmp = mul(u.crumple, .9);
			const seedOff = local(mul(u.seed, 7.31), "seedOff");
			const neps = .012;
			const cr0 = local(crumpleField(pdx, pdy, cScale, seedOff), "cr0");
			const crx = div(sub(crumpleField(add(pdx, neps), pdy, cScale, seedOff), cr0), neps);
			const cry = div(sub(crumpleField(pdx, add(pdy, neps), cScale, seedOff), cr0), neps);
			const coverage = clamp(u.crumpleCoverage, 0, 1);
			const covNoise = add(mul(surfaceNoise(add(add(mul(pdx, 2.6), seedOff), 31.7), add(add(mul(pdy, 2.6), mul(seedOff, .617)), 17.3)), .5), .5);
			const covThreshold = local(sub(1.25, mul(coverage, 1.75)), "covThreshold");
			const crumpleW = local(mul(mul(cAmp, smoothstep(sub(covThreshold, .25), add(covThreshold, .25), covNoise)), add(mul(smoothstep(0, .35, depthNorm), .85), .15)), "crumpleW");
			const n = nudgeNormal(nBase, mul(mul(crx, crumpleW), .25), mul(mul(cry, crumpleW), .25));
			const grazing = grazingFlat(n);
			const R = local(reflect(vec3(0, 0, 1), n), "R");
			const rough = local(clamp(u.roughness, 0, 1), "rough");
			const rJit = local(mul(mul(rough, surfaceNoise(mul(pdx, 140), mul(pdy, 140))), .18), "rJit");
			const eu = local(add(add(mul(R.member("x"), lx), mul(R.member("y"), ly)), rJit), "eu");
			const ev = local(add(add(mul(R.member("x"), mul(ly, -1)), mul(R.member("y"), lx)), mul(rJit, .6)), "ev");
			const soft = local(mix(.02, .45, rough), "soft");
			const keyBank = mul(mul(envBox(eu, .18, .95, soft, "win1"), envBox(ev, -.42, .42, soft, "win2")), sub(1, mul(mul(envBox(ev, -.045, .045, mul(soft, .6), "mull"), .8), sub(1, rough))));
			const strip = mul(mul(envBox(eu, -.16, -.05, mul(soft, .7), "strip1"), envBox(ev, -.85, .85, soft, "strip2")), .85);
			const fill = mul(mul(envBox(eu, -.95, -.45, mul(soft, 1.6), "fill1"), envBox(ev, -.6, .6, mul(soft, 1.6), "fill2")), .3);
			const dome = mul(clamp(add(mul(eu, .5), .5), 0, 1), .12);
			const env = mul(add(add(add(keyBank, strip), fill), dome), sub(1, mul(rough, .55)));
			const fresnel = add(.3, mul(pow(grazing, 3), .7));
			const specW = mul(u.reflectivity, fresnel);
			const kl = keyLightAt(light, -.8);
			const L = kl.member("L");
			const specular = local(mul(add(env, mul(mul(pow(clamp(dot(n, kl.member("H")), 0, 1), exp2(mix(9, 4, rough))), 1.6), sub(1, mul(rough, .7)))), specW), "specular");
			const lit = local(add(mul(clamp(dot(n, L), -1, 1), .5), .5), "lit");
			const diffuse = mix(sub(1, mul(u.shading, .75)), add(1, mul(u.shading, .45)), lit);
			const ao = mix(.6, 1, smoothstep(0, 1, clamp(div(mul(sdf, -1), .025), 0, 1)));
			const rimGlow = mul(mul(mul(pow(grazing, 3.5), u.rim), add(mul(lit, .6), .4)), 1.6);
			const plasticRgb = local(add(add(mul(mul(baseRgb, diffuse), ao), splat3(specular)), mul(u.rimColor.member("rgb"), rimGlow)), "plasticRgb");
			return guarded(insideShape(sdf, pxH), vec4(plasticRgb, transition), ZERO, "plastic");
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
			description: "Center position of the plastic shape",
			ui: {
				type: "position",
				label: "Center",
				group: "Position",
				units: ["%", "px"]
			}
		},
		scale: {
			default: 1,
			description: "Scale of the plastic shape (1 = default size)",
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
			description: "Rotation of the plastic shape in degrees",
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
			default: "#f5f5f7",
			transform: transformColor,
			description: "Body color at the start of the gradient",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#d9dade",
			transform: transformColor,
			description: "Body color at the end of the gradient",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		stops: colorStopsPropConfig(),
		gradientAngle: {
			default: 90,
			description: "Direction of the body color gradient in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Gradient Angle",
				group: "Colors"
			}
		},
		colorSpace: {
			default: "linear",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space for gradient color interpolation",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		},
		thickness: {
			default: .5,
			description: "Puffiness — how far the rounded bevel reaches inward from the edge before the surface flattens",
			ui: {
				type: "range",
				min: .05,
				max: 1,
				step: .01,
				label: "Puffiness",
				group: "Plastic"
			}
		},
		roughness: {
			default: .12,
			description: "Surface roughness — 0 = polished gloss with crisp mirror reflections, 1 = matte satin",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Roughness",
				group: "Plastic"
			}
		},
		reflectivity: {
			default: 1,
			description: "Strength of the environment reflections and specular highlights",
			ui: {
				type: "range",
				min: 0,
				max: 2,
				step: .01,
				label: "Reflectivity",
				group: "Plastic"
			}
		},
		crumple: {
			default: .25,
			description: "Surface crumple — irregular dents that break reflections into hard organic shapes, like blown or vacuum-formed plastic",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Crumple",
				group: "Plastic"
			}
		},
		crumpleScale: {
			default: 1,
			description: "Size of the crumple dents — higher = smaller, denser dents",
			ui: {
				type: "range",
				min: .5,
				max: 10,
				step: .1,
				label: "Crumple Scale",
				group: "Plastic"
			}
		},
		crumpleCoverage: {
			default: 1,
			description: "How much of the surface is crumpled — 0 = pristine, 0.5 = patches of crumple, 1 = fully crumpled",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Crumple Coverage",
				group: "Plastic"
			}
		},
		seed: {
			default: 0,
			description: "Random seed — re-rolls the crumple pattern for reflection variety",
			ui: {
				type: "range",
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Plastic"
			}
		},
		edgeSoftness: {
			default: .05,
			description: "Softness of the shape boundary edge",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Edge Softness",
				group: "Plastic"
			}
		},
		lightAngle: {
			default: 315,
			description: "Direction the studio key light comes from, in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Light Angle",
				group: "Lighting"
			}
		},
		speed: {
			default: 1,
			description: "Speed of the environment drift — the studio reflections slowly orbit the surface. 0 pauses.",
			ui: {
				type: "range",
				min: 0,
				max: 4,
				step: .01,
				label: "Speed",
				group: "Animation"
			}
		},
		shading: {
			default: .35,
			description: "Diffuse shading contrast — how strongly the body darkens away from the light",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Shading",
				group: "Lighting"
			}
		},
		rim: {
			default: .25,
			description: "Fresnel rim light intensity on grazing surfaces",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Rim Light",
				group: "Lighting"
			}
		},
		rimColor: {
			default: "#ffffff",
			transform: transformColor,
			description: "Color of the fresnel rim light",
			ui: {
				type: "color",
				label: "Rim Color",
				group: "Lighting"
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
var Plastic_default = componentDefinition;
export { componentDefinition as n, Plastic_default as t };
