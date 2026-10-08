import { Ac as defineStd, Ki as noiseTone, Ml as p, Ui as gaborGrains, aa as unitized, ea as seededPlane, qo as stops, zo as rampOver } from "./std-ClftfWpF.js";
import { i as noiseToneProps, r as noiseSpeedProp, t as noiseColorProps } from "./noiseColor-D_bPhJ4m.js";
const componentDefinition = defineStd({
	name: "GaborNoise",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Oriented sine-grain noise with a fingerprint-like flow",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		...noiseColorProps(),
		scale: {
			default: 1.5,
			description: "Pattern scale (higher = finer, more detailed patterns; each +1 roughly doubles the frequency)",
			ui: {
				type: "range",
				min: -2,
				max: 5,
				step: .1,
				label: "Scale",
				group: "Effect"
			}
		},
		frequency: {
			default: 8,
			description: "Frequency of the oriented waves within each cell",
			ui: {
				type: "range",
				min: 1,
				max: 24,
				step: .5,
				label: "Frequency",
				group: "Effect"
			}
		},
		...noiseToneProps(),
		...noiseSpeedProp()
	},
	paint: rampOver(noiseTone(seededPlane(unitized(gaborGrains({ frequency: p("frequency") })), {
		scale: p("scale"),
		seed: p("seed")
	}), {
		contrast: p("contrast"),
		balance: p("balance")
	}), stops(p("colorSpace")))
});
var GaborNoise_default = componentDefinition;
export { componentDefinition as n, GaborNoise_default as t };
