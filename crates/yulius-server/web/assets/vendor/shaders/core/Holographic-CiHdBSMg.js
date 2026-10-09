import { Ac as defineStd, Bd as ZERO, Dc as vec3, Gs as clamp, Hr as fieldSlope, Ic as animatedTime, Js as dot, Oc as vec4, Rr as cosineRainbow, Ru as shapeEffectBoundingBoxDeclaration, Sc as splat3, Si as surfaceNoise, Tc as sub, Ws as add, Yr as guarded, Zr as insideShape, Zs as float, bc as smoothstep, dc as mul, hi as shapedSurface, mc as pow, oc as local, pc as normalize, qs as div, uc as mix, vi as silhouette, wi as surfacePattern, xi as surfaceField, yc as sin } from "./std-ClftfWpF.js";
import { c as transformPosition } from "./transformations-P50Uo_Ju.js";
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
var THIRD = 1 / 3;
var TWOTHIRD = 2 / 3;
var LIGHT_X = Math.cos(300 * Math.PI / 180);
var LIGHT_Y = Math.sin(300 * Math.PI / 180);
var L_X = .37164708971977234;
var L_Y = -.6437116265296936;
var L_Z = -.6689647436141968;
var holoRainbow = (t) => cosineRainbow(t, [
	0,
	THIRD,
	TWOTHIRD
]);
const componentDefinition = defineStd({
	name: "Holographic",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "Iridescent holographic foil sticker with animated rainbow sheen and glitter flakes",
	animatedTime: { speed: "speed" },
	...shapedSurface({
		pattern: "raw",
		stencil: {
			kind: "forward",
			eps: .008
		},
		surface: (frame, params) => {
			const { uniforms: u } = params;
			const t = local(animatedTime(params), "animTime");
			const vol = frame.volumetric;
			const EPS = .008;
			const field = surfaceField(frame, params, { scale: u.scale });
			const { s0, sX, sY, sdf, pxH } = field;
			const transition = silhouette(field, u.edgeSoftness);
			const suv = local(frame.sdfUV, "suv");
			const pat = surfacePattern(frame, field, { uv: suv });
			const pdx = pat.member("x");
			const pdy = pat.member("y");
			const qdx = sub(suv.member("x"), .5);
			const qdy = sub(suv.member("y"), .5);
			let baseNx;
			let baseNy;
			let steepAtten;
			if (vol) {
				const bx = local(clamp(div(sub(sX.member("w"), s0.member("w")), EPS), -1.5, 1.5), "baseNxVol");
				const by = local(clamp(div(sub(sY.member("w"), s0.member("w")), EPS), -1.5, 1.5), "baseNyVol");
				baseNx = bx;
				baseNy = by;
				steepAtten = local(div(1, add(1, mul(clamp(add(mul(bx, bx), mul(by, by)), 0, 2), 2))), "steepAtten");
			} else {
				const grad = fieldSlope(field, EPS);
				const domeSlope = local(mul(sub(1, clamp(div(mul(sdf, -1), .06), 0, 1)), .35), "domeSlope");
				baseNx = local(mul(grad.member("x"), domeSlope), "baseNx");
				baseNy = local(mul(grad.member("y"), domeSlope), "baseNy");
				steepAtten = float(1);
			}
			const crScale = local(mul(u.crinkleScale, 1.35), "crScale");
			const crAmp = local(mul(mul(u.crinkle, .8), steepAtten), "crAmp");
			const neps = .02;
			const w0 = local(surfaceNoise(add(mul(pdx, crScale), 2.7), add(mul(pdy, crScale), 11.3)), "w0");
			const wx = div(sub(surfaceNoise(add(mul(add(pdx, neps), crScale), 2.7), add(mul(pdy, crScale), 11.3)), w0), neps);
			const wy = div(sub(surfaceNoise(add(mul(pdx, crScale), 2.7), add(mul(add(pdy, neps), crScale), 11.3)), w0), neps);
			const n = local(normalize(vec3(add(baseNx, mul(mul(wx, crAmp), .4)), add(baseNy, mul(mul(wy, crAmp), .4)), -1)), "n");
			const fScale = local(mul(u.foilScale, 2.2), "fScale");
			const drift = local(mul(t, .06), "drift");
			const blotch = add(surfaceNoise(add(mul(pdx, fScale), mul(drift, LIGHT_X)), add(mul(pdy, fScale), mul(drift, LIGHT_Y))), mul(surfaceNoise(sub(add(mul(mul(pdx, fScale), 2.1), 5.2), mul(mul(drift, LIGHT_X), 1.6)), sub(add(mul(mul(pdy, fScale), 2.1), 9.4), mul(mul(drift, LIGHT_Y), 1.6))), .4));
			const normalTilt = add(mul(n.member("x"), LIGHT_X), mul(n.member("y"), LIGHT_Y));
			const ramp = mul(add(mul(pdx, LIGHT_X), mul(pdy, LIGHT_Y)), .8);
			const hue = local(add(add(add(add(mul(blotch, .45), ramp), mul(normalTilt, 1.4)), mul(t, .18)), div(u.hueShift, 360)), "hue");
			const saturationC = clamp(u.saturation, 0, 1);
			const spectral = holoRainbow(hue);
			const foilTint = local(mix(vec3(.72, .73, .76), spectral, splat3(mul(saturationC, .85))), "foilTint");
			const roughnessC = local(clamp(u.roughness, 0, 1), "roughnessC");
			const L = local(vec3(L_X, L_Y, L_Z), "L");
			const H = local(normalize(add(L, vec3(0, 0, -1))), "H");
			const ndl = local(clamp(add(mul(dot(n, L), .5), .5), 0, 1), "ndl");
			const ndh = local(clamp(dot(n, H), 0, 1), "ndh");
			const sheen = mul(mul(pow(ndh, 7), .45), sub(1, mul(roughnessC, .35)));
			const specCore = mul(mul(pow(ndh, mix(float(42), float(12), roughnessC)), .85), sub(1, mul(roughnessC, .55)));
			const specRgb = add(mul(foilTint, sheen), mul(mix(foilTint, vec3(1, 1, 1), splat3(.6)), specCore));
			const flakeN = surfaceNoise(mul(pdx, 190), mul(pdy, 190));
			const flakePhase = local(surfaceNoise(add(mul(pdx, 190), 47.3), sub(mul(pdy, 190), 31.1)), "flakePhase");
			const twinkle = local(add(mul(sin(add(mul(t, 2.2), mul(flakePhase, 12.6))), .5), .5), "twinkle");
			const flakeMask = mul(mul(smoothstep(.52, .62, add(mul(flakeN, .75), mul(twinkle, .3))), clamp(u.sparkle, 0, 1)), steepAtten);
			const flakeRgb = mul(mul(holoRainbow(add(add(hue, .33), mul(flakePhase, .5))), add(.85, mul(twinkle, .55))), add(mul(ndl, .6), .7));
			const grainMul = add(1, mul(mul(surfaceNoise(mul(qdx, 420), mul(qdy, 420)), roughnessC), .28));
			const foilRgb = local(mul(mix(add(mul(foilTint, add(mul(ndl, .5), .62)), specRgb), flakeRgb, splat3(flakeMask)), grainMul), "foilRgb");
			return guarded(insideShape(sdf, pxH), vec4(foilRgb, transition), ZERO, "holo");
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
			description: "Center position of the foil shape",
			ui: {
				type: "position",
				label: "Center",
				group: "Position",
				units: ["%", "px"]
			}
		},
		scale: {
			default: 1,
			description: "Scale of the foil shape (1 = default size)",
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
			description: "Rotation of the foil shape in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Position"
			}
		},
		hueShift: {
			default: 0,
			description: "Rotates the rainbow spectrum across the foil",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Hue Shift",
				group: "Foil"
			}
		},
		foilScale: {
			default: 1.5,
			description: "Size of the iridescent color patches — higher = smaller, busier patches",
			ui: {
				type: "range",
				min: .3,
				max: 6,
				step: .01,
				label: "Foil Scale",
				group: "Foil"
			}
		},
		saturation: {
			default: .75,
			description: "Rainbow saturation — 0 = plain silver foil, 1 = fully spectral",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Saturation",
				group: "Foil"
			}
		},
		roughness: {
			default: .25,
			description: "Matte laminate grain — softens the gloss with fine paper-like noise",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Roughness",
				group: "Foil"
			}
		},
		speed: {
			default: 1,
			description: "Animation speed — the spectrum drifts across the foil as if the sticker is tilting in your hand. 0 pauses.",
			ui: {
				type: "range",
				min: 0,
				max: 2,
				step: .01,
				label: "Speed",
				group: "Animation"
			}
		},
		crinkle: {
			default: .5,
			description: "Laminate wrinkles — perturbs the surface so the sheen and rainbow swirl locally instead of lying flat",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Crinkle",
				group: "Foil"
			}
		},
		crinkleScale: {
			default: 1,
			description: "Size of the laminate wrinkles — higher = smaller, denser wrinkles",
			ui: {
				type: "range",
				min: .3,
				max: 6,
				step: .01,
				label: "Crinkle Scale",
				group: "Foil"
			}
		},
		sparkle: {
			default: .4,
			description: "Glitter flakes embedded in the foil — tiny facets that refract a shifted hue and twinkle as the spectrum drifts",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Sparkle",
				group: "Foil"
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
				group: "Foil"
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
var Holographic_default = componentDefinition;
export { componentDefinition as n, Holographic_default as t };
