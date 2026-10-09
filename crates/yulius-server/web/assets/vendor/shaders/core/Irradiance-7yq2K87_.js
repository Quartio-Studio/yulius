import { Ac as defineStd, Br as exposureTone, Dc as vec3, Ec as vec2, Er as emissiveAlpha, Fl as glassShellProps, Gr as geometricNormal, Hr as fieldSlope, Js as dot, Lr as continuedField, Oc as vec4, Qr as inverseSquare, Ru as shapeEffectBoundingBoxDeclaration, Sc as splat3, Tc as sub, Ws as add, Yr as guarded, Zr as insideShape, Zs as float, ai as nearestEdge, bc as smoothstep, cc as max, dc as mul, di as pointLightFrom, eu as listPropConfig, fd as isMobileGpuViewport, gr as shadowVisibility, hi as shapedSurface, mc as pow, mr as irradianceField, nc as gt, o as accumulate, oc as local, pc as normalize, qs as div, s as listOf, ti as lambert, uc as mix, ui as placementPoint, vi as silhouette, xi as surfaceField } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, i as transformBoolean } from "./transformations-P50Uo_Ju.js";
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
var DEFAULT_LIGHTS = [{
	position: {
		x: .3,
		y: .3
	},
	color: "#ffb347",
	intensity: 3
}, {
	position: {
		x: .72,
		y: .7
	},
	color: "#7a5cff",
	intensity: 2
}];
var MAX_LIGHTS = 6;
var RAYS = {
	burst: 128,
	motion: 64,
	refine: 32,
	refineFrames: 12
};
var MOBILE_RAY_DIVISOR = 2;
var MARCH_STEPS = 32;
var SHADOW_STEPS = 12;
var FIELD_RES = 512;
var FIELD_RES_MOBILE = 320;
var CORE_WIDTH_PX = 2.5;
var STENCIL_EPS = .01;
var BODY_AMBIENT = .3;
var GLINT_EXPONENT = 60;
var GLINT_GAIN = .6;
function gatherSettings(propValues) {
	const shadowsRaw = propValues.shadows;
	const shadows = typeof shadowsRaw === "number" ? shadowsRaw > 0 : shadowsRaw !== false;
	const mobile = isMobileGpuViewport();
	const div$1 = mobile ? MOBILE_RAY_DIVISOR : 1;
	return {
		rays: {
			burst: RAYS.burst / div$1,
			motion: RAYS.motion / div$1,
			refine: RAYS.refine / div$1,
			refineFrames: RAYS.refineFrames
		},
		shadowSteps: shadows ? SHADOW_STEPS : 0,
		resolution: mobile ? FIELD_RES_MOBILE : FIELD_RES
	};
}
var GATHER_PROPS = {
	lights: {
		prop: "lights",
		position: "position",
		color: "color",
		intensity: "intensity"
	},
	reach: "reach",
	wrap: "wrap",
	lightRange: "lightRange",
	shadowSoftness: "shadowSoftness",
	steps: MARCH_STEPS
};
var spine = shapedSurface({
	stencil: "centre",
	centreTap: "fast",
	surface: (frame, params) => {
		const { uniforms: u, propValues } = params;
		const { shadowSteps } = gatherSettings(propValues);
		const shadows = shadowSteps > 0;
		const field = surfaceField(frame, params);
		const stencilled = (hint) => ({
			...field,
			s0: local(frame.sampler(frame.sdfUV), `${hint}S0`),
			sX: local(frame.gradSampler(add(frame.sdfUV, vec2(STENCIL_EPS, 0))), `${hint}X`),
			sY: local(frame.gradSampler(add(frame.sdfUV, vec2(0, STENCIL_EPS))), `${hint}Y`)
		});
		const px = local(div(field.pxH, max(u.scale, .001)), "pxUV");
		const far = continuedField(frame, field, params);
		const lights = listOf(params, "lights");
		const lightAt = (i) => {
			const item = lights.at(i);
			return {
				uv: placementPoint(params, item.position("position"), "lightUV"),
				rgb: mul(item.color("color").member("rgb"), item.number("intensity"))
			};
		};
		const irradiance = irradianceField({
			...gatherSettings(propValues),
			...GATHER_PROPS
		}).sample(params) ?? splat3(0);
		const rim = local(sub(1, smoothstep(0, mul(px, CORE_WIDTH_PX), far.sdf)), "rim");
		const rimLight = () => {
			const edge = nearestEdge(frame.sdfUV, far.sdf, add(fieldSlope(stencilled("rim"), STENCIL_EPS), far.outward));
			return accumulate(lights, (i) => {
				const light = lightAt(i);
				const onEdge = pointLightFrom({
					position: light.uv,
					height: u.lightHeight
				}, edge.point, "rimLight");
				let struck = mul(lambert(edge.normal, onEdge.toLight, { wrap: u.wrap }), inverseSquare(onEdge.distance, u.lightRange));
				if (shadows) struck = mul(struck, shadowVisibility({
					from: add(edge.point, mul(edge.normal, mul(px, 2))),
					toward: light.uv,
					field: far.at,
					steps: SHADOW_STEPS,
					surfaceEps: px,
					softness: u.shadowSoftness,
					hint: "rimVisibility"
				}));
				return mul(light.rgb, struck);
			}, {
				zero: "vec3f",
				hint: "rimSum",
				deps: [edge.point, edge.normal]
			});
		};
		const glow = emissiveAlpha(exposureTone(add(irradiance, guarded(gt(rim, 0), mul(rimLight(), mul(rim, u.core)), splat3(0), "emitter", [px]))), "glow");
		const shadeBody = () => {
			const n = geometricNormal(frame, stencilled("body"), {
				bevelWidth: u.bevelWidth,
				bevelShape: float(0),
				eps: STENCIL_EPS
			});
			const lit = accumulate(lights, (i) => {
				const light = lightAt(i);
				const onBody = pointLightFrom({
					position: light.uv,
					height: u.lightHeight
				}, frame.sdfUV, "bodyLight");
				const bodyLit = local(mul(inverseSquare(onBody.distance, u.lightRange), u.bodyLight), "bodyLit");
				const glint = mul(pow(max(dot(n, normalize(add(onBody.L, vec3(0, 0, -1)))), 0), GLINT_EXPONENT), GLINT_GAIN);
				return add(mul(u.bodyColor.member("rgb"), mul(lambert(n, onBody.L), bodyLit)), mul(light.rgb, mul(glint, bodyLit)));
			}, {
				zero: "vec3f",
				hint: "bodySum",
				deps: [n]
			});
			return exposureTone(add(mul(u.bodyColor.member("rgb"), BODY_AMBIENT), lit));
		};
		const body = guarded(insideShape(field.sdf, px), shadeBody(), splat3(0), "body", []);
		const cov = local(silhouette({
			...field,
			pxH: px
		}, u.edgeSoftness), "cov");
		return vec4(mix(glow.member("rgb"), body, cov), mix(glow.member("a"), 1, cov));
	}
});
const componentDefinition = defineStd({
	name: "Irradiance",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "Photorealistic light spilling around the edge of any 2D, SVG, or 3D shape — any number of movable colored point lights strike the silhouette and the lit edges irradiate their surroundings, gathered in a compute pass with real cast shadows, mixing where they meet and burning to white at the rim",
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
		lights: listPropConfig({
			maxItems: MAX_LIGHTS,
			minItems: 1,
			itemLabel: "Light",
			item: {
				position: {
					kind: "position",
					default: {
						x: .3,
						y: .3
					},
					label: "Position",
					description: "Where this light sits — the edges facing it are struck and spill light outward. Map it to the mouse to carry the light around the shape."
				},
				color: {
					kind: "color",
					default: "#ffb347",
					label: "Color",
					description: "The color this light casts"
				},
				intensity: {
					kind: "number",
					default: 3,
					label: "Intensity",
					min: 0,
					max: 8,
					step: .01,
					description: "Brightness of this light — past ~2 the rim it strikes overexposes to white"
				}
			}
		}, {
			default: DEFAULT_LIGHTS,
			description: "The point lights striking the shape — each with its own position, color and brightness",
			label: "Lights",
			group: "Lights"
		}),
		lightHeight: {
			default: .5,
			description: "Height of the lights above the shape plane, relative to the shape — low lights graze the body, high lights flood it evenly",
			ui: {
				type: ["range", "map"],
				min: .05,
				max: 3,
				step: .01,
				label: "Light Height",
				group: "Light"
			}
		},
		lightRange: {
			default: 1.2,
			description: "Reach of each light — the distance at which it has fallen to half strength, relative to the shape",
			ui: {
				type: ["range", "map"],
				min: .1,
				max: 4,
				step: .01,
				label: "Range",
				group: "Light"
			}
		},
		reach: {
			default: 3,
			description: "How far from the shape the spilled light is gathered, relative to the shape — the glow fades out toward this distance",
			ui: {
				type: ["range", "map"],
				min: .5,
				max: 8,
				step: .01,
				label: "Reach",
				group: "Glow"
			}
		},
		core: {
			default: 1,
			description: "The lit edge seen directly — the white-hot line along the rim where the lights strike",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 2,
				step: .01,
				label: "Core",
				group: "Glow"
			}
		},
		wrap: {
			default: .1,
			description: "How far light wraps around edges that face away from it — 0 leaves the far side dark",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Wrap",
				group: "Glow"
			}
		},
		shadows: {
			default: true,
			compileTime: true,
			transform: transformBoolean,
			description: "Cast shadows — the body blocks each light from edges it stands in front of (keeps holes and concave pockets dark)",
			ui: {
				type: "checkbox",
				label: "Shadows",
				group: "Glow"
			}
		},
		shadowSoftness: {
			default: .15,
			description: "Penumbra width of the cast shadows — 0 is razor sharp",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Shadow Softness",
				group: "Glow"
			}
		},
		bodyColor: {
			default: "#000000",
			transform: transformColor,
			description: "Color of the shape itself — black for a pure occluder, lighter to see the lights land on its form",
			ui: {
				type: "color",
				label: "Body Color",
				group: "Body"
			}
		},
		bodyLight: {
			default: .6,
			description: "How much the lights illuminate the body — the bevel of a flat shape or the form of a 3D one",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 2,
				step: .01,
				label: "Body Light",
				group: "Body"
			}
		},
		bevelWidth: {
			default: .01,
			description: "Width of the rounded edge a flat shape catches the light on, relative to the shape",
			ui: {
				type: ["range", "map"],
				min: .005,
				max: .2,
				step: .001,
				label: "Bevel Width",
				group: "Body"
			}
		},
		...glassShellProps({ edgeSoftness: { ui: { group: "Body" } } }),
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
	...spine,
	compute: (params) => irradianceField({
		...gatherSettings(params.propValues),
		...GATHER_PROPS
	}).compute(params)
});
var Irradiance_default = componentDefinition;
export { componentDefinition as n, Irradiance_default as t };
