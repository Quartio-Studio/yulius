import { Ac as defineStd, Ga as brickCourses, Ml as p, mo as strokeOver, vo as vary } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, r as transformAngle, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var field = brickCourses({
	cellsX: p("cellsX"),
	cellsY: p("cellsY"),
	mortar: p("mortar"),
	softness: p("softness"),
	variation: p("variation"),
	rotation: p("rotation"),
	offset: p("offset"),
	speedVariance: p("speedVariance"),
	seed: p("seed")
});
const componentDefinition = defineStd({
	name: "BrickPattern",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Classic brick wall pattern with alternating rows and mortar gaps",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		colorBrick: {
			default: "#000000",
			transform: transformColor,
			description: "Brick color",
			ui: {
				type: "color",
				label: "Brick Color",
				group: "Colors"
			}
		},
		colorMortar: {
			default: "#ffffff",
			transform: transformColor,
			description: "Mortar / gap color",
			ui: {
				type: "color",
				label: "Mortar Color",
				group: "Colors"
			}
		},
		cellsX: {
			default: 8,
			description: "Number of bricks per row",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 30,
				step: 1,
				label: "Bricks per Row",
				group: "Effect"
			}
		},
		cellsY: {
			default: 10,
			description: "Number of brick rows",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 30,
				step: 1,
				label: "Rows",
				group: "Effect"
			}
		},
		mortar: {
			default: .05,
			description: "Width of mortar gaps — equal pixel thickness in both directions",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Mortar Gap",
				group: "Effect"
			}
		},
		softness: {
			default: 0,
			description: "Softness of the brick edges (0 = crisp, 1 = very soft)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Softness",
				group: "Effect"
			}
		},
		variation: {
			default: 0,
			description: "Per-brick random lightening/darkening of the brick color",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Variation",
				group: "Effect"
			}
		},
		rotation: {
			default: 0,
			transform: transformAngle,
			description: "Rotation of the pattern in degrees",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Effect"
			}
		},
		speed: {
			default: 0,
			description: "Animation speed",
			ui: {
				type: "range",
				min: -2,
				max: 2,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		},
		offset: {
			default: 0,
			description: "Static horizontal offset — shifts the brick pattern without animating",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Offset",
				group: "Animation"
			}
		},
		speedVariance: {
			default: 0,
			description: "How much each row's speed varies — at high values rows move at different speeds and directions",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Speed Variance",
				group: "Animation"
			}
		},
		seed: {
			default: 0,
			description: "Random seed for per-row speed variation",
			ui: {
				type: "range",
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Animation"
			}
		},
		colorSpace: {
			default: "linear",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space for interpolation",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		}
	},
	paint: strokeOver({
		fill: p("colorMortar"),
		stroke: vary(p("colorBrick"), field.shade),
		mask: field.bricks,
		space: p("colorSpace")
	})
});
var BrickPattern_default = componentDefinition;
export { componentDefinition as n, BrickPattern_default as t };
