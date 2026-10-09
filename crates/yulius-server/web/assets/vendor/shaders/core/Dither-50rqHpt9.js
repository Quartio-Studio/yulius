import { Ac as defineStd, Ml as p, Xa as ditherInks, co as quantise, so as pixelGrid } from "./std-ClftfWpF.js";
import { a as transformColor } from "./transformations-P50Uo_Ju.js";
var transformPattern = (value) => {
	return {
		bayer2: 0,
		bayer4: 1,
		bayer8: 2,
		clusteredDot: 3,
		blueNoise: 4,
		whiteNoise: 5,
		floydSteinberg: 6
	}[value] ?? 1;
};
var transformColorMode = (value) => value === "source" ? 1 : 0;
var grid = pixelGrid({ size: p("pixelSize") });
const componentDefinition = defineStd({
	name: "Dither",
	role: "filter",
	species: "gather",
	category: "Stylize",
	description: "Dithering effect with multiple pattern options",
	blendWithChildren: false,
	boundingBoxDeclaration: { aspectRatio: null },
	props: {
		pattern: {
			default: "bayer4",
			transform: transformPattern,
			compileTime: true,
			description: "Dithering pattern algorithm",
			ui: {
				type: "select",
				options: [
					{
						label: "Bayer 2x2",
						value: "bayer2"
					},
					{
						label: "Bayer 4x4",
						value: "bayer4"
					},
					{
						label: "Bayer 8x8",
						value: "bayer8"
					},
					{
						label: "Clustered Dot",
						value: "clusteredDot"
					},
					{
						label: "Blue Noise",
						value: "blueNoise"
					},
					{
						label: "White Noise",
						value: "whiteNoise"
					},
					{
						label: "Floyd-Steinberg",
						value: "floydSteinberg"
					}
				],
				label: "Pattern",
				group: "Effect"
			}
		},
		pixelSize: {
			default: 4,
			description: "Size of dithering pixels",
			ui: {
				type: "range",
				min: 1,
				max: 20,
				step: 1,
				label: "Pixel Size",
				group: "Effect"
			}
		},
		threshold: {
			default: .5,
			description: "Luminance threshold for dithering",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Threshold",
				group: "Effect"
			}
		},
		spread: {
			default: 1,
			description: "How much of the luminance range participates in dithering (lower = more solid areas)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Spread",
				group: "Effect"
			}
		},
		colorMode: {
			default: "custom",
			transform: transformColorMode,
			compileTime: true,
			description: "How colors are determined",
			ui: {
				type: "select",
				options: [{
					label: "Custom Colors",
					value: "custom"
				}, {
					label: "Source Colors",
					value: "source"
				}],
				label: "Color Mode",
				group: "Colors"
			}
		},
		colorA: {
			default: "transparent",
			transform: transformColor,
			description: "Dark color for dithering",
			ui: {
				type: "color",
				label: "Dark Color",
				condition: { colorMode: "custom" },
				group: "Colors"
			}
		},
		colorB: {
			default: "#ffffff",
			transform: transformColor,
			description: "Light color for dithering",
			ui: {
				type: "color",
				label: "Light Color",
				condition: { colorMode: "custom" },
				group: "Colors"
			}
		}
	},
	effect: ditherInks({
		grid,
		levels: quantise({
			grid,
			pattern: p("pattern"),
			threshold: p("threshold"),
			spread: p("spread")
		}),
		mode: p("colorMode"),
		colors: [p("colorA"), p("colorB")]
	})
});
var Dither_default = componentDefinition;
export { componentDefinition as n, Dither_default as t };
