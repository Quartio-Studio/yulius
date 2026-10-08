import { Ac as defineStd, Ka as cellFrame, Ml as p, go as triangleLines, mo as strokeOver, vo as vary } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, r as transformAngle, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var field = triangleLines({
	frame: cellFrame({
		cells: p("cells"),
		rotation: p("rotation"),
		convention: "flippedY"
	}),
	thickness: p("thickness"),
	softness: p("softness"),
	variation: p("variation"),
	speedVariance: p("speedVariance")
});
const componentDefinition = defineStd({
	name: "TriangularGrid",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Tiling grid of equilateral triangles with optional animated row offsets",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		colorA: {
			default: "#1a1a1a",
			transform: transformColor,
			description: "Triangle fill color",
			ui: {
				type: "color",
				label: "Fill",
				group: "Colors"
			}
		},
		colorB: {
			default: "#ffffff",
			transform: transformColor,
			description: "Grid line color",
			ui: {
				type: "color",
				label: "Lines",
				group: "Colors"
			}
		},
		cells: {
			default: 8,
			description: "Number of triangle rows down the shortest canvas edge",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 40,
				step: 1,
				label: "Cells",
				group: "Effect"
			}
		},
		thickness: {
			default: 1,
			description: "Thickness of the grid lines (0 = no lines)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 10,
				step: .1,
				label: "Thickness",
				group: "Effect"
			}
		},
		rotation: {
			default: 0,
			transform: transformAngle,
			description: "Rotation of the grid in degrees",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Effect"
			}
		},
		softness: {
			default: 0,
			description: "Softness of the line edges (0 = crisp, 1 = very soft)",
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
			description: "Per-triangle random lightening/darkening of the fill color",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Variation",
				group: "Effect"
			}
		},
		speed: {
			default: 0,
			description: "Animates the triangle rows drifting horizontally (0 = static)",
			ui: {
				type: "range",
				min: -3,
				max: 3,
				step: .01,
				label: "Speed",
				group: "Animation"
			}
		},
		speedVariance: {
			default: .3,
			description: "Per-row random speed variance for irregular drifting motion",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Speed Variance",
				group: "Animation"
			}
		},
		colorSpace: {
			default: "linear",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space for color interpolation",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		}
	},
	paint: strokeOver({
		fill: vary(p("colorA"), field.shade),
		stroke: p("colorB"),
		mask: field.lines,
		space: p("colorSpace")
	})
});
var TriangularGrid_default = componentDefinition;
export { componentDefinition as n, TriangularGrid_default as t };
