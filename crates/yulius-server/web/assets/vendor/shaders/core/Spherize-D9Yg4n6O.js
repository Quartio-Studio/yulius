import { $t as crispTap, Ac as defineStd, Ml as p, Mn as sphereFrame, sn as gatherStack, wn as rimLit } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition } from "./transformations-P50Uo_Ju.js";
var sphere = sphereFrame({
	center: p("center"),
	radius: p("radius"),
	depth: p("depth")
});
const componentDefinition = defineStd({
	name: "Spherize",
	role: "filter",
	species: "gather",
	category: "Distortions",
	description: "Map content onto a 3D sphere surface with depth distortion",
	boundingBoxDeclaration: { aspectRatio: null },
	props: {
		radius: {
			default: 1,
			description: "Radius of the sphere (1 = half viewport height)",
			ui: {
				type: ["range", "map"],
				min: .1,
				max: 3,
				step: .1,
				label: "Radius",
				group: "Effect"
			}
		},
		depth: {
			default: 1,
			description: "How much the sphere bulges toward viewer (0 = flat, higher = more bulge)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 3,
				step: .1,
				label: "Depth",
				group: "Effect"
			}
		},
		center: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "The center point of the sphere",
			ui: {
				type: "position",
				label: "Center",
				group: "Position"
			}
		},
		lightPosition: {
			default: {
				x: .3,
				y: .3
			},
			transform: transformPosition,
			description: "Position of the specular light source",
			ui: {
				type: "position",
				label: "Light Position",
				group: "Position"
			}
		},
		lightIntensity: {
			default: .5,
			description: "Intensity of the rim light (0 = off)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Light Intensity",
				group: "Effect"
			}
		},
		lightSoftness: {
			default: .5,
			description: "Softness of the rim light falloff (0 = hard edge, 1 = soft glow)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Light Softness",
				group: "Effect"
			}
		},
		lightColor: {
			default: "#ffffff",
			transform: transformColor,
			description: "Color of the specular highlight",
			ui: {
				type: "color",
				label: "Light Color",
				group: "Effect"
			}
		}
	},
	effect: gatherStack(crispTap(sphere), [rimLit(sphere, {
		position: p("lightPosition"),
		intensity: p("lightIntensity"),
		softness: p("lightSoftness"),
		color: p("lightColor")
	})], { resultAlpha: "straight" })
});
var Spherize_default = componentDefinition;
export { componentDefinition as n, Spherize_default as t };
