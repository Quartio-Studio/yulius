import { Ac as defineStd, Fi as borderOverlay, Ii as cellBorders, Li as cellDistances, Ml as p, Ri as cellFill, Wo as share, ou as colorStopsPropConfig, qo as stops, zo as rampOver } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var cells = share(cellDistances({
	scale: p("scale"),
	seed: p("seed")
}));
const componentDefinition = defineStd({
	name: "Voronoi",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Cellular pattern where each pixel is colored by its distance to the nearest of many scattered points",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		colorA: {
			default: "#3186cf",
			transform: transformColor,
			description: "Color near each cell's center point",
			ui: {
				type: "color",
				label: "Color A (Center)",
				group: "Colors"
			}
		},
		colorB: {
			default: "#fc02dd",
			transform: transformColor,
			description: "Color at cell boundaries, far from any center point",
			ui: {
				type: "color",
				label: "Color B (Edge)",
				group: "Colors"
			}
		},
		stops: colorStopsPropConfig(),
		colorBorder: {
			default: "#000000",
			transform: transformColor,
			description: "Color of the cell boundary lines",
			ui: {
				type: "color",
				label: "Border Color",
				group: "Colors"
			}
		},
		scale: {
			default: 6,
			description: "Number of cells across the canvas",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 20,
				step: .5,
				label: "Scale",
				group: "Effect"
			}
		},
		speed: {
			default: .5,
			description: "Animation speed — how fast the cell points drift",
			ui: {
				type: "range",
				min: 0,
				max: 5,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		},
		seed: {
			default: 0,
			description: "Random seed — shifts the cell pattern without changing the overall structure",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Effect"
			}
		},
		edgeIntensity: {
			default: .5,
			description: "Controls how much of the cell interior is filled by the edge color. Low = center color dominates with a sharp boundary. High = edge color spreads further into the cell.",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Edge Intensity",
				group: "Effect"
			}
		},
		edgeSoftness: {
			default: .05,
			description: "Width of the cell boundary lines.",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: .4,
				step: .005,
				label: "Edge Softness",
				group: "Effect"
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
	paint: borderOverlay(rampOver(share(cellFill(cells, { edgeIntensity: p("edgeIntensity") })), stops(p("colorSpace"))), cellBorders(cells, {
		softness: p("edgeSoftness"),
		scale: p("scale")
	}), { color: p("colorBorder") })
});
var Voronoi_default = componentDefinition;
export { componentDefinition as n, Voronoi_default as t };
