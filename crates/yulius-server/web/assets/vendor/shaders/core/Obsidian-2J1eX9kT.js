import { Ac as defineStd, Ai as viewRay, Bd as ZERO, Fl as glassShellProps, Gr as geometricNormal, Ic as animatedTime, Jr as grazingOf, Ml as p, Mo as foldRamp, Oc as vec4, Ru as shapeEffectBoundingBoxDeclaration, Sc as splat3, Tc as sub, Ws as add, Yr as guarded, Zr as insideShape, ar as direction, bc as smoothstep, bi as studioSoftboxes, dc as mul, hc as reflect, hi as shapedSurface, mi as schlickFresnel, oc as local, or as directionFrame, ou as colorStopsPropConfig, qo as stops, uc as mix, vi as silhouette, wi as surfacePattern, xi as surfaceField } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
var DEFAULT_STOPS = [
	{
		color: "#ff2f7a",
		position: 0
	},
	{
		color: "#2a2cff",
		position: .3
	},
	{
		color: "#5fe6ff",
		position: .55
	},
	{
		color: "#a6ffd8",
		position: .75
	},
	{
		color: "#ffb3e6",
		position: 1
	}
];
var ANGLE_SHIFT = .9;
var FLOW_CYCLE = .55;
var GLASS_SKY_GAIN = .25;
const componentDefinition = defineStd({
	name: "Obsidian",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "Dark tinted glass whose faces stay near-black while every surface turning away from the viewer ignites with a flowing iridescent gradient — vivid color living only on the oblique walls and bevels",
	animatedTime: { speed: "speed" },
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
			description: "Center position of the glass shape",
			ui: {
				type: "position",
				label: "Center",
				group: "Position",
				units: ["%", "px"]
			}
		},
		scale: {
			default: 1,
			description: "Scale of the glass shape (1 = default size)",
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
			description: "Rotation of the glass shape in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Position"
			}
		},
		bodyColor: {
			default: "#06071a",
			transform: transformColor,
			description: "The dark body of the glass — what the faces looking straight at you show. Keep it deep for the iridescence to pop.",
			ui: {
				type: "color",
				label: "Body Color",
				group: "Colors"
			}
		},
		colorA: {
			default: "#ff2f7a",
			transform: transformColor,
			description: "First iridescent color (two-color fallback when stops are cleared)",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#5fe6ff",
			transform: transformColor,
			description: "Second iridescent color (two-color fallback when stops are cleared)",
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
			description: "Color space for color interpolation",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		},
		iridescence: {
			default: 1,
			description: "Strength of the color on the oblique surfaces — 0 = plain dark glass, above 1 = saturated neon walls",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1.5,
				step: .01,
				label: "Iridescence",
				group: "Iridescence"
			}
		},
		rimWidth: {
			default: .55,
			description: "How far in from edge-on the color reaches — small keeps it to the steepest walls, large lets it creep across gently turning faces",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Rim Width",
				group: "Iridescence"
			}
		},
		rimSoftness: {
			default: .25,
			description: "Softness of the transition from dark face to colored wall",
			ui: {
				type: ["range", "map"],
				min: .02,
				max: .6,
				step: .01,
				label: "Rim Softness",
				group: "Iridescence"
			}
		},
		flowAngle: {
			default: 30,
			description: "Direction the gradient flows across the surface, in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Flow Angle",
				group: "Iridescence"
			}
		},
		flowScale: {
			default: 1,
			description: "How many times the palette cycles across the shape — higher = tighter color bands",
			ui: {
				type: ["range", "map"],
				min: .2,
				max: 4,
				step: .01,
				label: "Flow Scale",
				group: "Iridescence"
			}
		},
		speed: {
			default: .5,
			description: "Speed the gradient flows along the walls. 0 pauses.",
			ui: {
				type: "range",
				min: 0,
				max: 4,
				step: .01,
				label: "Speed",
				group: "Iridescence"
			}
		},
		gloss: {
			default: .35,
			description: "Strength of the studio softboxes reflected in the glass — the pale highlights riding the top edges",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 2,
				step: .01,
				label: "Gloss",
				group: "Surface"
			}
		},
		...glassShellProps({ envRotation: { ui: { group: "Surface" } } }),
		bevelWidth: {
			default: .08,
			description: "Width of the edge bevel on flat shapes — the band that turns away from the viewer and catches the color",
			ui: {
				type: ["range", "map"],
				min: .005,
				max: .3,
				step: .001,
				label: "Bevel Width",
				group: "Surface"
			}
		},
		bevelShape: {
			default: 0,
			description: "Bevel profile — 0 = one smooth round fillet, 1 = machined: a steep outer fillet, a chamfer plateau and an inner knee",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Bevel Profile",
				group: "Surface"
			}
		},
		...glassShellProps({ edgeSoftness: { ui: { group: "Surface" } } }),
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
	...shapedSurface({
		pattern: "raw",
		surface: (frame, params) => {
			const { uniforms: u } = params;
			const t = animatedTime(params);
			const field = surfaceField(frame, params, { scale: u.scale });
			const n = geometricNormal(frame, field, {
				bevelWidth: u.bevelWidth,
				bevelShape: u.bevelShape
			});
			const view = viewRay(params, field);
			const grazing = grazingOf(n, view);
			const rimEdge = local(sub(1, u.rimWidth), "rimEdge");
			const rim = local(smoothstep(sub(rimEdge, u.rimSoftness), add(rimEdge, u.rimSoftness), grazing), "rim");
			const along = directionFrame(u.flowAngle, "flow").coordsOf(surfacePattern(frame, field)).along;
			const phase = add(add(mul(along, mul(u.flowScale, 1 / FLOW_CYCLE)), mul(grazing, ANGLE_SHIFT)), mul(t, .2));
			const iri = local(stops(p("colorSpace"))(foldRamp(phase), params).member("rgb"), "iri");
			const R = local(reflect(view, n), "R");
			const shine = local(mul(studioSoftboxes(R.member("x"), R.member("y"), {
				rotation: direction(u.envRotation, "env"),
				keyRadius: .3,
				drift: 0,
				strength: u.gloss,
				skyGain: GLASS_SKY_GAIN
			}), schlickFresnel(grazing, {
				r0: .04,
				gain: .96,
				power: 4
			})), "shine");
			const rgb = add(mix(u.bodyColor.member("rgb"), iri, splat3(mul(rim, u.iridescence))), mul(mix(iri, splat3(1), splat3(.6)), splat3(mul(shine, .5))));
			return guarded(insideShape(field.sdf, field.pxH), vec4(rgb, silhouette(field, u.edgeSoftness)), ZERO, "obsidian");
		}
	})
});
var Obsidian_default = componentDefinition;
export { componentDefinition as n, Obsidian_default as t };
