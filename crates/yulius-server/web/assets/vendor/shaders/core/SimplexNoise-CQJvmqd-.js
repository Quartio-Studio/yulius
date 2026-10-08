import { Ac as defineStd, Fo as noiseField, Ml as p, Vi as evolving, ea as seededPlane, qo as stops, ta as signedTone, zo as rampOver } from "./std-ClftfWpF.js";
import { t as noiseColorProps } from "./noiseColor-D_bPhJ4m.js";
const componentDefinition = defineStd({
	name: "SimplexNoise",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Organic noise with animated movement",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		...noiseColorProps(),
		scale: {
			default: 2,
			description: "Pattern scale (higher = larger patterns)",
			ui: {
				type: "range",
				min: -2,
				max: 5,
				step: .1,
				label: "Scale",
				group: "Effect"
			}
		},
		balance: {
			default: 0,
			description: "Balance between colors (negative = more colorB, positive = more colorA)",
			ui: {
				type: ["range", "map"],
				min: -1,
				max: 1,
				step: .1,
				label: "Balance",
				group: "Effect"
			}
		},
		contrast: {
			default: 0,
			description: "Pattern contrast (higher = sharper transitions)",
			ui: {
				type: ["range", "map"],
				min: -2,
				max: 5,
				step: .1,
				label: "Contrast",
				group: "Effect"
			}
		},
		seed: {
			default: 0,
			description: "Random seed for pattern variation",
			ui: {
				type: "range",
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Effect"
			}
		},
		speed: {
			default: 1,
			description: "Animation speed",
			ui: {
				type: "range",
				min: 0,
				max: 5,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		}
	},
	paint: rampOver(signedTone(seededPlane(evolving(noiseField("mx3signed"), { rate: .5 }), {
		scale: p("scale"),
		seed: p("seed")
	}), {
		contrast: p("contrast"),
		balance: p("balance")
	}), stops(p("colorSpace")))
});
var SimplexNoise_default = componentDefinition;
export { componentDefinition as n, SimplexNoise_default as t };
