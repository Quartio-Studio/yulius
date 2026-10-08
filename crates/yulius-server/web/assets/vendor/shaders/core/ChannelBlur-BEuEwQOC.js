import { Ac as defineStd, Ml as p, Yt as channelBlur } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "ChannelBlur",
	role: "filter",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Blurs",
	description: "Independent blur for red, green, and blue channels",
	requiresRTT: true,
	requiresChild: true,
	props: {
		redIntensity: {
			default: 0,
			description: "Blur intensity for red channel",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Red Intensity",
				group: "Effect"
			}
		},
		greenIntensity: {
			default: 20,
			description: "Blur intensity for green channel",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Green Intensity",
				group: "Effect"
			}
		},
		blueIntensity: {
			default: 40,
			description: "Blur intensity for blue channel",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Blue Intensity",
				group: "Effect"
			}
		}
	},
	...channelBlur({
		red: p("redIntensity"),
		green: p("greenIntensity"),
		blue: p("blueIntensity")
	})
});
var ChannelBlur_default = componentDefinition;
export { componentDefinition as n, ChannelBlur_default as t };
