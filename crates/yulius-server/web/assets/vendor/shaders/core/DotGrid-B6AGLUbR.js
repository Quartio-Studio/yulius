import { Ac as defineStd, Ml as p, Za as dotLattice, bo as withAlpha, do as sampleMapsAtCellCentres } from "./std-ClftfWpF.js";
import { a as transformColor } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "DotGrid",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Grid of dots with optional twinkling animation",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		color: {
			default: "#ffffff",
			description: "The color of the dot",
			transform: transformColor,
			ui: {
				type: "color",
				label: "Color",
				group: "Colors"
			}
		},
		density: {
			default: 30,
			description: "The number of dots along the canvas height (the width fits as many as the aspect ratio allows)",
			ui: {
				type: "range",
				min: 1,
				max: 200,
				step: 1,
				label: "Density",
				group: "Effect"
			}
		},
		dotSize: {
			default: .3,
			description: "The size of each dot, zero (0) being invisible, one (1) filled the grid with no gaps",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Dot Size",
				group: "Effect"
			}
		},
		offset: {
			default: 0,
			description: "Horizontal stagger of alternating rows (0.5 = classic polka-dot brick offset)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Row Offset",
				group: "Effect"
			}
		},
		speed: {
			default: 0,
			description: "Animates the rows drifting horizontally (0 = static)",
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
		twinkle: {
			default: 0,
			description: "Intensity of the twinkle effect (0 = off, 1 = full twinkle)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .1,
				label: "Twinkle",
				group: "Animation"
			}
		}
	},
	mapSampleUVs: sampleMapsAtCellCentres({
		cells: p("density"),
		props: ["dotSize", "twinkle"]
	}),
	paint: withAlpha(p("color"), dotLattice({
		density: p("density"),
		dotSize: p("dotSize"),
		offset: p("offset"),
		speedVariance: p("speedVariance"),
		twinkle: p("twinkle")
	}))
});
var DotGrid_default = componentDefinition;
export { componentDefinition as n, DotGrid_default as t };
