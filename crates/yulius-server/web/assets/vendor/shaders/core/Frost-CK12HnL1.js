import { $r as keyLightAt, Ac as defineStd, Bd as ZERO, Cc as sqrt, Dc as vec3, Ec as vec2, Fr as beerLambert, Gs as clamp, Hd as call, Hr as fieldSlope, Ic as animatedTime, Js as dot, Oc as vec4, Ru as shapeEffectBoundingBoxDeclaration, Sc as splat3, Si as surfaceNoise, Tc as sub, Ws as add, Yr as guarded, Zr as insideShape, Zs as float, ar as direction, bc as smoothstep, cc as max, ci as opticalThickness, dc as mul, ff as mxWorleyNoiseFloat2Pub, hi as shapedSurface, mc as pow, nc as gt, oc as local, pc as normalize, qr as grazingFlat, qs as div, si as nudgeNormal, uc as mix, vi as silhouette, wi as surfacePattern, xi as surfaceField, yc as sin } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition } from "./transformations-P50Uo_Ju.js";
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
const componentDefinition = defineStd({
	name: "Frost",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "Photoreal frozen ice — thickness-driven subsurface scattering that reads as a solid block, with fine frost crystals creeping in from the edges",
	animatedTime: { speed: "speed" },
	...shapedSurface({
		pattern: "raw",
		chord: "firstLobe",
		surface: (frame, params) => {
			const { uniforms: u } = params;
			const t = animatedTime(params);
			const vol = frame.volumetric;
			const EPS = .01;
			const field = surfaceField(frame, params, { scale: u.scale });
			const { s0, sX, sY, sdf, pxH } = field;
			const grad = fieldSlope(field);
			const transition = silhouette(field, u.edgeSoftness);
			const pat = surfacePattern(frame, field);
			const pdx = pat.member("x");
			const pdy = pat.member("y");
			const depthNorm = local(clamp(div(mul(sdf, -1), max(.3, .01)), 0, 1), "depthNorm");
			let nBase;
			let steepAtten;
			if (vol) {
				const ddx = local(clamp(div(sub(sX.member("w"), s0.member("w")), EPS), -4, 4), "ddx");
				const ddy = local(clamp(div(sub(sY.member("w"), s0.member("w")), EPS), -4, 4), "ddy");
				nBase = local(normalize(vec3(ddx, ddy, -1)), "nBase");
				steepAtten = local(div(1, add(1, mul(clamp(add(mul(ddx, ddx), mul(ddy, ddy)), 0, 2), 2))), "steepAtten");
			} else {
				const bevelSlope = local(mul(sqrt(max(sub(1, mul(depthNorm, depthNorm)), 0)), 1.8), "bevelSlope");
				nBase = local(normalize(vec3(mul(grad.member("x"), bevelSlope), mul(grad.member("y"), bevelSlope), -1)), "nBase");
				steepAtten = float(1);
			}
			const thickness = local(mul(opticalThickness(field, frame), u.density), "thickness");
			const depthT = local(clamp(mul(thickness, 1.2), 0, 1), "depthT");
			const transmit = local(beerLambert(thickness, mul(vec3(1.6, .85, .45), u.absorption)), "transmit");
			const scatteredRgb = local(mul(mix(vec3(1, 1, 1), u.iceColor.member("rgb"), splat3(depthT)), transmit), "scatteredRgb");
			const frostField = local(mul(sub(1, smoothstep(0, max(mul(u.frostDepth, .3), .01), mul(sdf, -1))), u.frostAmount), "frostField");
			const frostScale = local(mul(u.frostScale, 60), "frostScl");
			const feps = .004;
			const fn0 = local(surfaceNoise(mul(pdx, frostScale), mul(pdy, frostScale)), "fn0");
			const fnx = div(sub(surfaceNoise(mul(add(pdx, feps), frostScale), mul(pdy, frostScale)), fn0), feps);
			const fny = div(sub(surfaceNoise(mul(pdx, frostScale), mul(add(pdy, feps), frostScale)), fn0), feps);
			const frostNormalAmp = local(mul(mul(frostField, u.frostRoughness), steepAtten), "frostNormalAmp");
			const wScale = mul(u.frostScale, 40);
			const wc = call(mxWorleyNoiseFloat2Pub, "mxWorleyNoiseFloat2Pub", [vec2(mul(pdx, wScale), mul(pdy, wScale)), float(1)]);
			const fx3 = local(guarded(gt(frostField, 0), vec3(mul(mul(fnx, frostNormalAmp), .03), mul(mul(fny, frostNormalAmp), .03), mul(smoothstep(.25, 0, wc), frostField)), vec3(0, 0, 0), "frostFx", [
				pat,
				frostField,
				frostScale,
				steepAtten
			]), "fx3");
			const n = nudgeNormal(nBase, fx3.member("x"), fx3.member("y"));
			const kl = keyLightAt(direction(u.lightAngle, "light"), -.6);
			const L = kl.member("L");
			const ndl = local(dot(n, L), "ndl");
			const ndh = local(clamp(dot(n, kl.member("H")), 0, 1), "ndh");
			const sssDiffuse = pow(add(mul(ndl, .5), .5), .7);
			const forwardGlow = mul(mul(mul(transmit, clamp(add(mul(dot(L, vec3(0, 0, 1)), .5), .5), 0, 1)), u.scatter), sub(1, mul(depthT, .4)));
			const sssRgb = add(mul(scatteredRgb, add(mul(sssDiffuse, .6), .5)), mul(forwardGlow, .5));
			const glintField = local(add(mul(surfaceNoise(add(mul(pdx, 190), 13.7), sub(mul(pdy, 190), 4.3)), .5), .5), "glintField");
			const twinkle = add(mul(sin(add(mul(t, 3), mul(glintField, 40))), .5), .5);
			const sparkleGlint = guarded(gt(frostField, 0), mul(mul(mul(smoothstep(.72, .96, mul(glintField, twinkle)), add(mul(pow(ndh, 6), .6), .4)), frostField), u.sparkle), float(0), "sparkleGlint", [
				pat,
				frostField,
				ndh
			]);
			const frostRgb = mul(vec3(.95, .98, 1), add(mul(fx3.member("z"), .4), mul(sparkleGlint, 4)));
			const fres = mul(pow(grazingFlat(n), 3), u.fresnel);
			const fresnelRgb = mul(vec3(.85, .93, 1), mul(fres, 1.5));
			const broadSpec = local(mul(mul(pow(ndh, 16), .4), u.gloss), "broadSpec");
			const iceRgb = local(add(add(add(add(sssRgb, mul(vec3(.4, .46, .55), u.ambient)), splat3(broadSpec)), fresnelRgb), frostRgb), "iceRgb");
			return guarded(insideShape(sdf, pxH), vec4(iceRgb, transition), ZERO, "frost");
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
			description: "Center position of the ice shape",
			ui: {
				type: "position",
				label: "Center",
				group: "Position",
				units: ["%", "px"]
			}
		},
		scale: {
			default: 1,
			description: "Scale of the ice shape (1 = default size)",
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
			description: "Rotation of the ice shape in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Position"
			}
		},
		iceColor: {
			default: "#0597fc",
			transform: transformColor,
			description: "Ice tint — the color deep ice saturates toward (thin edges stay near white)",
			ui: {
				type: "color",
				label: "Ice Color",
				group: "Ice"
			}
		},
		ambient: {
			default: 0,
			description: "Cool ambient fill so shadowed ice never goes black",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Ambient",
				group: "Ice"
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
				group: "Ice"
			}
		},
		density: {
			default: 5,
			description: "Optical depth multiplier — how thick and deep the ice reads",
			ui: {
				type: "range",
				min: 0,
				max: 10,
				step: .01,
				label: "Density",
				group: "Subsurface"
			}
		},
		absorption: {
			default: 1,
			description: "Beer–Lambert absorption strength — higher drives a deeper blue-teal core",
			ui: {
				type: "range",
				min: 0,
				max: 2,
				step: .01,
				label: "Absorption",
				group: "Subsurface"
			}
		},
		scatter: {
			default: .5,
			description: "Forward back-lit glow transmitted through thin ice",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Scatter",
				group: "Subsurface"
			}
		},
		frostAmount: {
			default: .8,
			description: "Overall density of the rim frost crystals",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Frost",
				group: "Frost"
			}
		},
		frostDepth: {
			default: .4,
			description: "How far the frost creeps inward from the rim",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Frost Depth",
				group: "Frost"
			}
		},
		frostScale: {
			default: 1,
			description: "Crystal size — higher = finer, busier frost",
			ui: {
				type: "range",
				min: .3,
				max: 4,
				step: .01,
				label: "Frost Scale",
				group: "Frost"
			}
		},
		frostRoughness: {
			default: 3,
			description: "Feathery roughness of the frost crystal surface",
			ui: {
				type: "range",
				min: 1,
				max: 5,
				step: .01,
				label: "Frost Roughness",
				group: "Frost"
			}
		},
		sparkle: {
			default: .5,
			description: "Tiny bright crystal glints catching the light at the rim",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Sparkle",
				group: "Frost"
			}
		},
		gloss: {
			default: .6,
			description: "Surface sheen — the broad soft icy specular",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Gloss",
				group: "Surface"
			}
		},
		fresnel: {
			default: .02,
			description: "Cold rim glow on grazing surfaces",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Fresnel",
				group: "Surface"
			}
		},
		lightAngle: {
			default: 300,
			description: "Key light direction in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Light Angle",
				group: "Surface"
			}
		},
		speed: {
			default: .3,
			description: "Sparkle twinkle speed. 0 freezes the sparkle.",
			ui: {
				type: "range",
				min: 0,
				max: 2,
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
			description: "URL to a pre-generated SDF .bin file"
		},
		shapeType: {
			default: "",
			compileTime: true,
			description: "Active SDF shape type"
		}
	}
});
var Frost_default = componentDefinition;
export { componentDefinition as n, Frost_default as t };
