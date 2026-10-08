import { Ac as defineStd, Ml as p, Nl as centerPropConfig, Pl as edgesPropConfig, Zn as perspective } from "./std-ClftfWpF.js";
import { c as transformPosition } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "Perspective",
	role: "warp",
	category: "Distortions",
	description: "Rotate the plane in 3D space with pan and tilt",
	boundingBoxDeclaration: { aspectRatio: null },
	props: {
		center: centerPropConfig("Center point of rotation"),
		pan: {
			default: 0,
			description: "Horizontal rotation (left/right)",
			ui: {
				type: ["range", "map"],
				min: -90,
				max: 90,
				step: .1,
				label: "Pan",
				group: "Effect"
			}
		},
		tilt: {
			default: 0,
			description: "Vertical rotation (up/down)",
			ui: {
				type: ["range", "map"],
				min: -90,
				max: 90,
				step: .1,
				label: "Tilt",
				group: "Effect"
			}
		},
		fov: {
			default: 60,
			description: "Field of view - controls perspective intensity",
			ui: {
				type: ["range", "map"],
				min: 30,
				max: 120,
				step: 1,
				label: "FOV",
				group: "Effect"
			}
		},
		zoom: {
			default: 1,
			description: "Zoom in to fill the frame after rotation",
			ui: {
				type: ["range", "map"],
				min: .5,
				max: 3,
				step: .1,
				label: "Zoom",
				group: "Effect"
			}
		},
		offset: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "Shift the result in X/Y",
			ui: {
				type: "position",
				label: "Offset",
				group: "Position"
			}
		},
		edges: edgesPropConfig("transparent", "How to handle edges")
	},
	map: perspective({
		center: p("center"),
		pan: p("pan"),
		tilt: p("tilt"),
		fov: p("fov"),
		zoom: p("zoom"),
		offset: p("offset")
	})
});
var Perspective_default = componentDefinition;
export { componentDefinition as n, Perspective_default as t };
